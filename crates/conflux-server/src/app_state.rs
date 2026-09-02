//! Single-experiment state (ADR 0003 — no multi-tenancy): one `AppState`
//! per server process, wiring together every library crate from Phases
//! 1–4. Every family here still ships exactly one member (`FedAvg`,
//! `GaussianClippingPrivacy`, `UniformRandomSelector`, `CosineScorer`), so
//! this phase wires them concretely rather than through
//! `conflux-config`'s `inventory` registry — see
//! its phase brief's scope note.
//!
//! `registry`/`store` are `Arc<AnyRegistry>`/`Arc<AnyStore>` —
//! see `backend_selection.rs` for how a caller picks which backend each
//! resolves to.

use conflux_net::TrustedReferenceTransport;
use conflux_net::jwt::JwtKeyMaterial;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use conflux_buffer::RoundBuffer;
use conflux_config::{Mode, ResolvedConfig};
use conflux_core::Aggregator;
use conflux_privacy::{PrivacyAccountant, PrivacyMechanism, RdpAccountant};
use conflux_proto::TaskResponse;
use conflux_registry::{
    AnyNodeAllowlist, AnyRegistry, InMemoryNodeAllowlist, InMemoryRegistry, RedisNodeAllowlist,
    RedisRegistry,
};
use conflux_reputation::CosineScorer;
use conflux_selector::{ClientSelector, SelectionSeed};
use conflux_store::{AnyStore, InMemoryStore, PostgresStore, PrivacyRoundLog, S3Store, StoreError};
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::broadcast;

use crate::backend_selection::{
    AccountingBackend, BackendSelection, BackendSelectionError, RegistryBackend, StoreBackend,
    validate_production_backends,
};
use crate::round_health::RoundLoopHealth;

#[derive(Debug, thiserror::Error)]
/// Why the server's shared state could not be assembled at startup.
pub enum AppStateError {
    #[error(transparent)]
    /// The requested backend combination is not permitted — production
    /// with in-memory storage, most often.
    BackendSelection(#[from] BackendSelectionError),
    #[error(transparent)]
    /// The registry backend could not be reached.
    Registry(#[from] conflux_registry::RegistryError),
    #[error(transparent)]
    /// The checkpoint backend could not be reached.
    Store(#[from] StoreError),
    #[error(transparent)]
    /// The node allow-list backend could not be reached.
    NodeAuth(#[from] conflux_registry::NodeAuthError),
}

/// Everything one experiment's server needs, constructed once at
/// startup and shared by every gRPC handler, HTTP handler, and the
/// round loop.
///
/// One process serves exactly one experiment (ADR 0003), which is why
/// nothing here is keyed by experiment id.
pub struct AppState {
    /// The fully resolved configuration, including the topology and mode
    /// it was resolved from.
    pub config: ResolvedConfig,
    /// Client lifecycle — who is registered and still alive.
    pub registry: Arc<AnyRegistry>,
    /// Where checkpoints are read from and written to.
    pub store: Arc<AnyStore>,
    /// always constructed, even when `config.require_node_auth`
    /// is `false` — toggling the parameter is then just a config change
    /// and a restart, not a wiring change, matching how every other
    /// startup-only config value in this codebase works.
    pub node_allowlist: Arc<AnyNodeAllowlist>,
    /// constructed by name from `config.selector.value` /
    /// `config.aggregator.value` via each family's own `build_*`
    /// function (`conflux-config`'s `inventory` registry, ADR 0002) —
    /// `Box<dyn _>` rather than a concrete type since the constructed
    /// type is only known at runtime.
    pub selector: Box<dyn ClientSelector>,
    /// The aggregation method, constructed by name from the resolved
    /// config.
    pub aggregator: Box<dyn Aggregator>,
    /// The trusted-reference sidecar connection (ADR 0011), when the
    /// configured aggregator needs one.
    ///
    /// `None` for every deployment that is not running a `trusted`-family
    /// method, which is all of them by default — the sidecar is an
    /// optional process, and a deployment that has not configured one
    /// never opens this connection or enters the code path that uses it.
    ///
    /// `tokio::sync::Mutex`, not `std::sync::Mutex`: the transport's
    /// calls are `async` and the guard is held across an `.await`, which
    /// the std guard cannot be. Every other `Mutex` in this struct
    /// guards purely synchronous state and stays `std`.
    ///
    /// Note the type: `conflux-server` holds a *client* from
    /// `conflux-net`. It does not depend on `conflux-trusted-reference`
    /// and must not be made to — ADR 0011, following ADR 0010's
    /// precedent, and CI's `isolation` job checks it.
    pub trusted_reference: Option<TokioMutex<TrustedReferenceTransport>>,
    /// constructed by name from `config.privacy_mechanism.value`
    /// via `conflux_privacy::build_privacy_mechanism` — the third of the
    /// three spec §5 families now registry-wired.
    pub privacy: Box<dyn PrivacyMechanism>,
    /// Cumulative privacy loss. `Mutex` because recording a round mutates
    /// it and every handler shares one `AppState`.
    pub accountant: Mutex<RdpAccountant>,
    /// `Some` when the privacy accountant's round history is durable
    /// across restarts — `None` reproduces the original
    /// in-memory-only behavior exactly. Deliberately independent of
    /// `store`'s own backend choice: a deployment can persist accounting
    /// via Postgres while checkpointing to S3, say.
    pub accountant_log: Option<Arc<PostgresStore>>,
    /// The contribution scorer used by the optional pre-aggregation
    /// filter.
    pub reputation: CosineScorer,
    /// The round a `fetch_task` call should hand out and a `submit_delta`
    /// call should accept. `run_round` advances this only after a round's
    /// checkpoint lands — a delta for a round that isn't current is
    /// rejected, not silently accepted into the wrong batch.
    pub round: AtomicU64,
    /// The open round's staging buffer, or `None` between rounds.
    pub current_buffer: Mutex<Option<Arc<RoundBuffer>>>,
    /// Tier 5 (H2): what the round loop is doing, so `/health` can report
    /// it instead of a constant.
    ///
    /// Lives here rather than being threaded into [`crate::router`] as a
    /// third parameter, because every other handler already reaches its
    /// state this way and `/health` should not be the one exception.
    /// Constructed in every `AppState`, including the ones tests build
    /// that never run a round loop — those simply stay `Starting`, which
    /// is the truthful answer for a server whose loop never started.
    pub round_loop_health: Arc<RoundLoopHealth>,
    /// Push-mode subscribers (spec §3: `cross_silo`) get every new round's
    /// task broadcast to them; pull-mode clients just see it on their next
    /// `fetch_task`.
    pub push_sender: broadcast::Sender<TaskResponse>,
    /// the public key `register()` verifies `auth_token`
    /// against when `config.auth.value == AuthMode::Jwt`. `None` means
    /// none was supplied — which `verify_jwt_if_required` treats as
    /// permitted in research and refused in production, the same
    /// asymmetry `resolve_server_tls` applies to missing TLS material.
    ///
    /// Set through [`AppState::with_jwt_key`] rather than a constructor
    /// parameter, so every existing `AppState::new*` call site — and
    /// every test that uses one — is unaffected.
    pub jwt_key: Option<JwtKeyMaterial>,
}

impl AppState {
    /// Everything in-memory, no network backends — unchanged from,
    /// still the default every pre-Phase-8 test and call site uses.
    pub fn new(config: ResolvedConfig, initial_weights: Vec<f32>) -> Self {
        Self::assemble(
            config,
            Arc::new(AnyRegistry::InMemory(InMemoryRegistry::new())),
            Arc::new(AnyStore::InMemory(InMemoryStore::new(initial_weights))),
            Arc::new(AnyNodeAllowlist::InMemory(InMemoryNodeAllowlist::new())),
            RdpAccountant::new(),
            None,
        )
    }

    /// Like [`AppState::new`], but the privacy accountant's round history
    /// is durable across restarts: connects `PostgresStore`,
    /// replays any rounds it already holds into a fresh `RdpAccountant`
    /// *before* the server answers its first round, and keeps the handle
    /// so every future `record_round` also appends there. `registry`/
    /// `store` stay in-memory — use [`AppState::connect`] for full
    /// per-field backend selection.
    pub async fn new_with_persistent_accounting(
        config: ResolvedConfig,
        initial_weights: Vec<f32>,
        postgres_url: &str,
    ) -> Result<Self, StoreError> {
        // Matches `PostgresStore::connect`'s own default table name.
        Self::new_with_persistent_accounting_table(
            config,
            initial_weights,
            postgres_url,
            "conflux_checkpoints",
        )
        .await
    }

    /// Same as [`AppState::new_with_persistent_accounting`], but against a
    /// caller-chosen table rather than the default — this crate's own
    /// tests use it for the same reason `PostgresStore::connect_with_table`
    /// exists: `cargo test`'s parallel execution against one real,
    /// never-wiped Postgres needs per-test isolation, not a shared table.
    pub async fn new_with_persistent_accounting_table(
        config: ResolvedConfig,
        initial_weights: Vec<f32>,
        postgres_url: &str,
        table: impl Into<String>,
    ) -> Result<Self, StoreError> {
        let (accountant, accountant_log) = connect_accounting(postgres_url, table).await?;

        Ok(Self::assemble(
            config,
            Arc::new(AnyRegistry::InMemory(InMemoryRegistry::new())),
            Arc::new(AnyStore::InMemory(InMemoryStore::new(initial_weights))),
            Arc::new(AnyNodeAllowlist::InMemory(InMemoryNodeAllowlist::new())),
            accountant,
            accountant_log,
        ))
    }

    /// The general constructor: connects whichever backend
    /// `backends` selects for the registry, the store, and privacy
    /// accounting, independently. Fails fast — before connecting
    /// anything — if `mode = production` and any of the three still
    /// resolves to its in-memory/disabled default (see
    /// `backend_selection::validate_production_backends`).
    pub async fn connect(
        config: ResolvedConfig,
        mode: Mode,
        initial_weights: Vec<f32>,
        backends: BackendSelection,
    ) -> Result<Self, AppStateError> {
        validate_production_backends(mode, &backends)?;

        // The allow-list backend follows the registry backend's choice
        // rather than being a fully independent fourth axis — a
        // deliberate simplification (one fewer env var), documented in
        // its phase brief.
        let node_allowlist = match &backends.registry {
            RegistryBackend::Memory => AnyNodeAllowlist::InMemory(InMemoryNodeAllowlist::new()),
            RegistryBackend::Redis { url } => {
                AnyNodeAllowlist::Redis(RedisNodeAllowlist::connect(url).await?)
            }
        };

        let registry = match &backends.registry {
            RegistryBackend::Memory => AnyRegistry::InMemory(InMemoryRegistry::new()),
            RegistryBackend::Redis { url } => {
                AnyRegistry::Redis(RedisRegistry::connect(url).await?)
            }
        };

        let store = match &backends.store {
            StoreBackend::Memory => AnyStore::InMemory(InMemoryStore::new(initial_weights)),
            StoreBackend::Postgres { url } => {
                AnyStore::Postgres(PostgresStore::connect(url).await?)
            }
            StoreBackend::S3 {
                endpoint,
                bucket,
                access_key,
                secret_key,
            } => AnyStore::S3(
                S3Store::connect(endpoint, bucket.clone(), access_key, secret_key).await?,
            ),
        };

        let (accountant, accountant_log) = match &backends.accounting {
            AccountingBackend::Disabled => (RdpAccountant::new(), None),
            AccountingBackend::Postgres { url } => {
                let (accountant, log) = connect_accounting(url, "conflux_checkpoints").await?;
                (accountant, log)
            }
        };

        Ok(Self::assemble(
            config,
            Arc::new(registry),
            Arc::new(store),
            Arc::new(node_allowlist),
            accountant,
            accountant_log,
        ))
    }

    fn assemble(
        config: ResolvedConfig,
        registry: Arc<AnyRegistry>,
        store: Arc<AnyStore>,
        node_allowlist: Arc<AnyNodeAllowlist>,
        accountant: RdpAccountant,
        accountant_log: Option<Arc<PostgresStore>>,
    ) -> Self {
        let seed = match config.seed_mode.value {
            conflux_config::SeedMode::Fixed => {
                SelectionSeed::Fixed(config.seed_value.value.unwrap_or(42))
            }
            conflux_config::SeedMode::OsRandom => SelectionSeed::OsRandom,
        };
        let (push_sender, _receiver) = broadcast::channel(16);

        // every existing call site resolves `selector`/
        // `aggregator` through the builtin fallback ("uniform_random"/
        // "fedavg", `conflux-config/src/lib.rs`'s `resolve()`), so this
        // can never actually panic for any test or default deployment —
        // only an explicit override naming something unregistered would,
        // the same "startup-invariant, not a runtime Result" treatment
        // `main.rs` already gives config resolution itself. Keeping
        // `assemble` infallible preserves `AppState::new`'s exact
        // signature.
        let selector = conflux_selector::build_selector(&config.selector.value, seed)
            .expect("unknown selector in resolved config");
        let aggregator = conflux_core::build_aggregator(
            &config.aggregator.value,
            conflux_core::AggregatorParams {
                byzantine_fraction: config.robust_byzantine_fraction.value,
                clip_radius: config.clip_radius.value,
                // `Some` unconditionally: `conflux-config` has already
                // resolved these through its own chain and logged where
                // each came from (ADR 0007), so passing `None` here to
                // mean "use the paper's default" would put a second,
                // invisible default underneath the one the startup log
                // just reported.
                server_learning_rate: Some(config.server_learning_rate.value),
                server_tau: Some(config.server_tau.value),
                server_momentum: Some(config.server_momentum.value),
                fairness_q: Some(config.fairness_q.value),
                server_lipschitz: Some(config.server_lipschitz.value),
                scaffold_num_clients: Some(config.scaffold_num_clients.value),
                zeno_rho: Some(config.zeno_rho.value),
            },
        )
        .expect("unknown aggregator in resolved config");
        let privacy = conflux_privacy::build_privacy_mechanism(
            &config.privacy_mechanism.value,
            config.clip_norm.value,
            config.noise_multiplier.value,
        )
        .expect("unknown privacy mechanism in resolved config");

        Self {
            registry,
            store,
            node_allowlist,
            selector,
            aggregator,
            privacy,
            accountant: Mutex::new(accountant),
            accountant_log,
            reputation: CosineScorer,
            round: AtomicU64::new(1),
            current_buffer: Mutex::new(None),
            round_loop_health: Arc::new(RoundLoopHealth::new()),
            push_sender,
            jwt_key: None,
            trusted_reference: None,
            config,
        }
    }

    /// Attaches a trusted-reference sidecar connection (ADR 0011).
    ///
    /// A consuming builder, for the same reason `with_jwt_key` is one:
    /// this is optional, startup-only, and needed by a small minority of
    /// deployments, so it does not belong in `assemble`'s parameter list
    /// where every caller would have to pass `None`.
    ///
    /// `main.rs` calls this only when the resolved aggregator reports
    /// `requires_trusted_reference()`. A deployment running `fedavg`
    /// never connects to a sidecar even if one is running.
    pub fn with_trusted_reference(mut self, transport: TrustedReferenceTransport) -> Self {
        self.trusted_reference = Some(TokioMutex::new(transport));
        self
    }

    /// Attaches the JWT public key `register()` verifies against.
    ///
    /// A consuming builder rather than another `new*` parameter: there
    /// are already three constructors, all of which every existing
    /// caller and test uses unchanged, and threading an
    /// `Option<JwtKeyMaterial>` through all of them would churn every
    /// one of those call sites to express something only `main.rs`
    /// ever has.
    pub fn with_jwt_key(mut self, jwt_key: Option<JwtKeyMaterial>) -> Self {
        self.jwt_key = jwt_key;
        self
    }
}

async fn connect_accounting(
    postgres_url: &str,
    table: impl Into<String>,
) -> Result<(RdpAccountant, Option<Arc<PostgresStore>>), StoreError> {
    let log = PostgresStore::connect_with_table(postgres_url, table).await?;
    let mut accountant = RdpAccountant::new();
    for (noise_multiplier, sample_rate) in log.load_rounds().await? {
        accountant.record_round(noise_multiplier, sample_rate);
    }
    // always replay per-client history too, regardless of
    // which `accounting_scope` is currently configured — a deployment
    // that switches scope between restarts should never silently lose
    // whichever history it wasn't actively using at the time.
    for (client_id, rounds) in log.load_client_rounds().await? {
        for (noise_multiplier, sample_rate) in rounds {
            accountant.record_round_for_client(&client_id, noise_multiplier, sample_rate);
        }
    }
    Ok((accountant, Some(Arc::new(log))))
}
