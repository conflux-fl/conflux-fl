//! Resolves every Conflux FL configuration parameter — which aggregator,
//! which topology, how long a round waits for quorum, and so on — against
//! a fixed six-tier precedence chain, and remembers *which* tier produced
//! each value:
//!
//! ```text
//! builtin fallback -> topology profile -> mode profile
//!   -> experiment file -> env var -> CLI flag -> resolved value
//! ```
//!
//! Later tiers win. A topology (`cross_silo`, `cross_device`,
//! `crowdsource`, `edge`) answers "what kind of participants and
//! network?" and owns a handful of connection-shaped parameters; a mode
//! (`research` or `production`) answers "am I iterating, or running a
//! live deployment?" and owns a disjoint set of safety-posture
//! parameters — the two axes never fight over the same field. Above
//! that, an experiment file, environment variables (`CONFLUX_*`), and
//! CLI flags let a specific run override either axis's defaults, in that
//! order of precedence.
//!
//! Tracking *where* a value came from (see [`Resolved`]) is what turns
//! "why is this deployment behaving strangely?" into a startup-log lookup
//! instead of a source-reading exercise — [`ResolvedConfig::to_log_lines`]
//! is what a caller (`conflux-server`) prints at startup, one line per
//! parameter, before doing anything else.
//!
//! This crate also hosts the compile-time strategy registry (see
//! [`registry`]) that lets an aggregator/selector/privacy-mechanism
//! implementation in another crate become selectable by name from
//! config, without this crate ever importing that other crate.

mod file;
mod registry;
mod source;
mod types;

pub use file::{ConfigFileError, load_experiment_file};
pub use registry::{StrategyEntry, StrategyKind, lookup};
pub use source::ConfigSource;
pub use types::{
    AccountingScope, AuthMode, BudgetExhaustedAction, ConnectionMode, LogFormat, Mode,
    ModeDefaults, SeedMode, Topology, TopologyDefaults,
};

use source::{LoggedValue, log_line};

/// A resolved config value paired with where it came from.
///
/// Generic over the value's type `T` so the same struct represents a
/// resolved `bool`, `f64`, or `ConnectionMode` alike — `ResolvedConfig`
/// below is nineteen fields of `Resolved<SomeType>`, not nineteen
/// hand-rolled "value + source" pairs.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolved<T> {
    pub value: T,
    pub source: ConfigSource,
}

/// One override tier's worth of parameters — every field is `Option<T>`:
/// `None` means "this tier has no opinion," `Some(v)` means "this tier
/// says `v`." `file`, `env`, and `cli` in [`resolve`] are each an
/// `Overrides` — same shape, different precedence. Field names match
/// their `CONFLUX_<NAME>` environment variable (uppercased) and their
/// `[experiment]` TOML key (as-is); allowed values come from each field's
/// own type (an enum for closed-set choices like [`AuthMode`], a bare
/// numeric type where any value in range is accepted).
///
/// `#[serde(default)]` is what makes a *partial* file work: most
/// experiments override a handful of parameters, and every key absent
/// from the file must resolve to `None` ("this tier has no opinion")
/// rather than failing to deserialize.
///
/// `deny_unknown_fields` is the deliberate counterpart. Serde's default
/// is to ignore keys it doesn't recognize, which would mean a typo —
/// `agregator = "krum"` — parses cleanly, resolves to the default
/// aggregator, and logs its source as a builtin fallback, with nothing
/// anywhere reporting that the file's instruction was dropped. ADR
/// 0007's principle is that a config value should always say where it
/// came from; a silently discarded key is the one case that can't. Better
/// to refuse the file and name the key.
#[derive(Debug, Default, Clone, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Overrides {
    pub connection_mode: Option<ConnectionMode>,
    pub auth: Option<AuthMode>,
    pub round_timeout_secs: Option<u64>,
    pub min_reputation_score: Option<f32>,
    pub client_registry_ttl: Option<u64>,
    pub quorum: Option<u32>,
    pub selector: Option<String>,
    pub seed_mode: Option<SeedMode>,
    pub seed_value: Option<u64>,
    pub aggregator: Option<String>,
    /// The assumed fraction of Byzantine (malicious or faulty) clients in
    /// a round's batch, `0.0..1.0`. Feeds the `robust` aggregator
    /// family's own math: Krum's *f* (how many updates it assumes could
    /// be attackers), Multi-Krum's *m* (how many it keeps), Trimmed
    /// Mean's trim count. Builtin fallback `0.2`. This is a per-algorithm
    /// tuning value, the same category as `clip_norm`/`noise_multiplier`
    /// below — not something a topology or mode should own an opinion
    /// on, since it depends on the deployment's actual threat model, not
    /// on whether participants are silos or phones.
    pub robust_byzantine_fraction: Option<f32>,
    /// Centered Clipping's clip radius `τ` — how far any one client's
    /// deviation from the running reference may pull the model in a
    /// round. Read only by the `centered_clipping` aggregator.
    ///
    /// **The builtin fallback of `1.0` is a placeholder, not a
    /// recommendation, and shipping against it is measurably worse than
    /// using no defense at all.** On real MNIST with a real
    /// 50,890-parameter MLP and one Byzantine client, `centered_clipping`
    /// at `τ = 1.0` scored 0.078 held-out accuracy where undefended
    /// `fedavg` scored 0.163 and `krum` scored 0.844
    /// (`docs/research/temporal-consistency-aggregation.md` §5.13).
    ///
    /// The reason is structural rather than a bad constant. `τ` bounds
    /// an L2 norm in *parameter space*, so it simultaneously bounds how
    /// far an attacker can pull the model **and** how far the model can
    /// move toward the truth — with the same number, spread across
    /// however many parameters the model has. Too small and the model
    /// cannot converge; large enough and nothing is clipped, at which
    /// point the method *is* FedAvg. A sweep on that model
    /// (1.0 → 5 → 20 → 100) rose monotonically toward FedAvg's own
    /// number with no optimum anywhere, while a sweep on a
    /// 3-dimensional synthetic model found a clear optimum at `τ = 4.0`.
    /// **`τ` does not transfer across model sizes**, so no default this
    /// field could carry would be right for an unknown model.
    ///
    /// Tune it against your own model, or use a selection-based robust
    /// aggregator, which has no equivalent parameter to get wrong.
    pub clip_radius: Option<f32>,
    /// Whether `conflux-reputation`'s pre-aggregation contribution filter
    /// runs at all, independent of which aggregator is selected. Builtin
    /// fallback `false`: every shipped aggregator's default behavior
    /// matches its cited paper exactly, with no framework-imposed
    /// filtering layered in front of it unless a deployment explicitly
    /// opts in. Like `robust_byzantine_fraction`, this is a deployment
    /// policy choice, not something topology/mode profiles set a default
    /// for.
    pub reputation_filter_enabled: Option<bool>,
    /// Whether `conflux-node` applies the configured privacy mechanism
    /// to a client's own update *before* it leaves the node, in addition
    /// to the server-side transform that always runs. Builtin fallback
    /// `false`, matching every other opt-in privacy/security posture
    /// here (`reputation_filter_enabled`, `require_node_auth`).
    ///
    /// The two application points answer different threat models rather
    /// than duplicating each other: client-side keeps a raw update from
    /// ever being observable in the clear by the network or the server
    /// (the `crowdsource`/`edge` case, where the server isn't fully
    /// trusted), server-side bounds the aggregate's exposure once
    /// batched. Turning this on adds a stage in front of the existing
    /// pipeline; it does not disable the server-side one.
    pub client_side_privacy_transform: Option<bool>,
    pub privacy_mechanism: Option<String>,
    pub clip_norm: Option<f32>,
    pub noise_multiplier: Option<f32>,
    pub target_epsilon: Option<f64>,
    pub delta: Option<f64>,
    pub budget_exhausted_action: Option<BudgetExhaustedAction>,
    pub accounting_scope: Option<AccountingScope>,
    pub allow_stub_client: Option<bool>,
    pub require_node_auth: Option<bool>,
    pub config_log_format: Option<LogFormat>,
}

/// Every configuration parameter, resolved against a topology + mode and
/// any overrides, with a [`ConfigSource`] attached to each one — this is
/// what makes resolution explainable rather than just a merged bag of
/// values: a caller can log or print exactly which tier produced each
/// field, not only its final value.
#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    /// The two axes this configuration was resolved *from*, carried on
    /// the result so a component holding a `ResolvedConfig` can answer
    /// "which deployment shape am I?" without the axes being passed
    /// alongside it everywhere.
    ///
    /// Not `Resolved<T>` like every field below, deliberately: these
    /// aren't layered values with a provenance, they're the inputs that
    /// *give* the other fields their provenance. Asking where `mode`
    /// came from is a question about the process's own arguments, not
    /// about config resolution.
    pub topology: Topology,
    pub mode: Mode,
    pub connection_mode: Resolved<ConnectionMode>,
    pub auth: Resolved<AuthMode>,
    pub round_timeout_secs: Resolved<u64>,
    pub min_reputation_score: Resolved<f32>,
    pub client_registry_ttl: Resolved<u64>,
    /// How many client updates a round needs before it closes. No
    /// built-in default exists for this one — `None` here means
    /// genuinely unset (no tier configured it), not "fell back to a
    /// built-in value"; a deployment must set it explicitly via a
    /// topology-appropriate value, an experiment file, `CONFLUX_QUORUM`,
    /// or `--quorum`.
    pub quorum: Option<Resolved<u32>>,
    pub selector: Resolved<String>,
    pub seed_mode: Resolved<SeedMode>,
    /// The fixed seed used for reproducible client sampling when
    /// `seed_mode` is `Fixed`. `None` when the resolved mode profile uses
    /// OS randomness instead (production's default) — the `source` still
    /// names which tier produced that `None`, same as any other resolved
    /// value.
    pub seed_value: Resolved<Option<u64>>,
    pub aggregator: Resolved<String>,
    pub robust_byzantine_fraction: Resolved<f32>,
    pub clip_radius: Resolved<f32>,
    pub reputation_filter_enabled: Resolved<bool>,
    pub client_side_privacy_transform: Resolved<bool>,
    pub privacy_mechanism: Resolved<String>,
    pub clip_norm: Resolved<f32>,
    pub noise_multiplier: Resolved<f32>,
    pub target_epsilon: Resolved<f64>,
    pub delta: Resolved<f64>,
    pub budget_exhausted_action: Resolved<BudgetExhaustedAction>,
    pub accounting_scope: Resolved<AccountingScope>,
    pub allow_stub_client: Resolved<bool>,
    pub require_node_auth: Resolved<bool>,
    pub config_log_format: Resolved<LogFormat>,
}

/// No variants today — every parameter [`resolve`] currently knows about
/// resolves unconditionally, so there's nothing left to fail on. Kept as
/// a real (if empty) type rather than removed, so `resolve()`'s
/// `Result<ResolvedConfig, ConfigError>` signature doesn't need to change
/// at every call site the moment a *future* parameter needs a genuine
/// resolve-time failure — a not-yet-implemented enum variant, an invalid
/// combination of two overrides, and so on. An empty enum is
/// uninstantiable, so a `Result<_, ConfigError>` is a compile-time
/// promise that resolution can't fail, until the day a variant is added
/// here and it can.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {}

/// Resolves one parameter against the six-tier precedence chain:
/// builtin fallback < topology profile < mode profile < file < env < cli.
/// The first tier (checked highest-precedence first) that set a value
/// wins.
///
/// This one generic function backs every field in [`resolve`] below,
/// rather than each of the ~20 parameters re-implementing the same
/// six-way precedence check by hand.
///
/// Ten arguments because there are genuinely six precedence tiers (each
/// needing a value) plus four of their sources — bundling them into a
/// struct would just move the same information around, not reduce it.
#[allow(clippy::too_many_arguments)]
fn layer<T: Clone>(
    builtin: T,
    topology: Option<T>,
    mode: Option<T>,
    file: Option<T>,
    env: Option<T>,
    cli: Option<T>,
    topology_source: &ConfigSource,
    mode_source: &ConfigSource,
    file_source: &ConfigSource,
    env_source: &ConfigSource,
) -> Resolved<T> {
    if let Some(value) = cli {
        return Resolved {
            value,
            source: ConfigSource::Cli,
        };
    }
    if let Some(value) = env {
        return Resolved {
            value,
            source: env_source.clone(),
        };
    }
    if let Some(value) = file {
        return Resolved {
            value,
            source: file_source.clone(),
        };
    }
    if let Some(value) = mode {
        return Resolved {
            value,
            source: mode_source.clone(),
        };
    }
    if let Some(value) = topology {
        return Resolved {
            value,
            source: topology_source.clone(),
        };
    }
    Resolved {
        value: builtin,
        source: ConfigSource::BuiltinFallback,
    }
}

/// Resolves every parameter against `topology` and `mode`'s defaults,
/// then layers `file` (an experiment file's overrides, if any), `env`,
/// and `cli` on top — highest precedence last.
///
/// Within that top "explicit override" tier, `cli` beats `env` beats
/// `file`: the most specific, most deliberately-typed-for-this-one-run
/// source wins over broader ones. A CLI flag is something you typed for
/// this exact invocation; an env var might be set globally in a shell
/// profile; a file's overrides might be checked into version control and
/// shared across many runs. Narrower scope, higher precedence.
pub fn resolve(
    topology: Topology,
    mode: Mode,
    file: Option<(&str, &Overrides)>,
    env: &Overrides,
    cli: &Overrides,
) -> Result<ResolvedConfig, ConfigError> {
    let topology_defaults = topology.defaults();
    let mode_defaults = mode.defaults();

    let topology_source = ConfigSource::TopologyProfile(topology.label().to_string());
    let mode_source = ConfigSource::ModeProfile(mode.label().to_string());
    let file_overrides = file.map(|(_, overrides)| overrides);
    let file_source = match file {
        Some((path, _)) => ConfigSource::ExperimentFile(path.to_string()),
        // Never read below: every field-level `file` extraction is `None`
        // whenever `file` itself is `None`, so this placeholder never
        // reaches a `Resolved`.
        None => ConfigSource::ExperimentFile(String::new()),
    };

    macro_rules! env_var {
        ($name:literal) => {
            ConfigSource::EnvVar(concat!("CONFLUX_", $name).to_string())
        };
    }

    let connection_mode = layer(
        topology_defaults.connection_mode,
        Some(topology_defaults.connection_mode),
        None,
        file_overrides.and_then(|o| o.connection_mode),
        env.connection_mode,
        cli.connection_mode,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CONNECTION_MODE"),
    );
    let auth = layer(
        topology_defaults.auth,
        Some(topology_defaults.auth),
        None,
        file_overrides.and_then(|o| o.auth),
        env.auth,
        cli.auth,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("AUTH"),
    );
    let round_timeout_secs = layer(
        topology_defaults.round_timeout_secs,
        Some(topology_defaults.round_timeout_secs),
        None,
        file_overrides.and_then(|o| o.round_timeout_secs),
        env.round_timeout_secs,
        cli.round_timeout_secs,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ROUND_TIMEOUT_SECS"),
    );
    let min_reputation_score = layer(
        topology_defaults.min_reputation_score,
        Some(topology_defaults.min_reputation_score),
        None,
        file_overrides.and_then(|o| o.min_reputation_score),
        env.min_reputation_score,
        cli.min_reputation_score,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("MIN_REPUTATION_SCORE"),
    );
    let client_registry_ttl = layer(
        topology_defaults.client_registry_ttl,
        Some(topology_defaults.client_registry_ttl),
        None,
        file_overrides.and_then(|o| o.client_registry_ttl),
        env.client_registry_ttl,
        cli.client_registry_ttl,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CLIENT_REGISTRY_TTL"),
    );

    // `quorum` has no builtin fallback, unlike every other parameter
    // above — handled separately from `layer` for exactly that reason.
    // `None` here means no tier set it at all, not "fell back to a
    // default"; the caller must supply it explicitly for a round to
    // ever close.
    let quorum = match (
        cli.quorum,
        env.quorum,
        file_overrides.and_then(|o| o.quorum),
    ) {
        (Some(value), _, _) => Some(Resolved {
            value,
            source: ConfigSource::Cli,
        }),
        (None, Some(value), _) => Some(Resolved {
            value,
            source: env_var!("QUORUM"),
        }),
        (None, None, Some(value)) => Some(Resolved {
            value,
            source: file_source.clone(),
        }),
        (None, None, None) => None,
    };

    let selector = layer(
        "uniform_random".to_string(),
        None,
        None,
        file_overrides.and_then(|o| o.selector.clone()),
        env.selector.clone(),
        cli.selector.clone(),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("SELECTOR"),
    );
    let seed_mode = layer(
        mode_defaults.seed_mode,
        None,
        Some(mode_defaults.seed_mode),
        file_overrides.and_then(|o| o.seed_mode),
        env.seed_mode,
        cli.seed_mode,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("SEED_MODE"),
    );
    let seed_value = layer(
        mode_defaults.seed_value,
        None,
        Some(mode_defaults.seed_value),
        file_overrides.and_then(|o| o.seed_value).map(Some),
        env.seed_value.map(Some),
        cli.seed_value.map(Some),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("SEED_VALUE"),
    );
    let aggregator = layer(
        "fedavg".to_string(),
        None,
        None,
        file_overrides.and_then(|o| o.aggregator.clone()),
        env.aggregator.clone(),
        cli.aggregator.clone(),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("AGGREGATOR"),
    );
    let robust_byzantine_fraction = layer(
        0.2_f32,
        None,
        None,
        file_overrides.and_then(|o| o.robust_byzantine_fraction),
        env.robust_byzantine_fraction,
        cli.robust_byzantine_fraction,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ROBUST_BYZANTINE_FRACTION"),
    );
    let clip_radius = layer(
        1.0_f32,
        None,
        None,
        file_overrides.and_then(|o| o.clip_radius),
        env.clip_radius,
        cli.clip_radius,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CLIP_RADIUS"),
    );
    let reputation_filter_enabled = layer(
        false,
        None,
        None,
        file_overrides.and_then(|o| o.reputation_filter_enabled),
        env.reputation_filter_enabled,
        cli.reputation_filter_enabled,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("REPUTATION_FILTER_ENABLED"),
    );
    let privacy_mechanism = layer(
        "gaussian_clipping".to_string(),
        None,
        None,
        file_overrides.and_then(|o| o.privacy_mechanism.clone()),
        env.privacy_mechanism.clone(),
        cli.privacy_mechanism.clone(),
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("PRIVACY_MECHANISM"),
    );
    let client_side_privacy_transform = layer(
        false,
        None,
        None,
        file_overrides.and_then(|o| o.client_side_privacy_transform),
        env.client_side_privacy_transform,
        cli.client_side_privacy_transform,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CLIENT_SIDE_PRIVACY_TRANSFORM"),
    );
    let clip_norm = layer(
        1.0_f32,
        None,
        None,
        file_overrides.and_then(|o| o.clip_norm),
        env.clip_norm,
        cli.clip_norm,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CLIP_NORM"),
    );
    let noise_multiplier = layer(
        1.0_f32,
        None,
        None,
        file_overrides.and_then(|o| o.noise_multiplier),
        env.noise_multiplier,
        cli.noise_multiplier,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("NOISE_MULTIPLIER"),
    );
    let target_epsilon = layer(
        8.0_f64,
        None,
        None,
        file_overrides.and_then(|o| o.target_epsilon),
        env.target_epsilon,
        cli.target_epsilon,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("TARGET_EPSILON"),
    );
    let delta = layer(
        1e-5_f64,
        None,
        None,
        file_overrides.and_then(|o| o.delta),
        env.delta,
        cli.delta,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("DELTA"),
    );
    let budget_exhausted_action = layer(
        mode_defaults.budget_exhausted_action,
        None,
        Some(mode_defaults.budget_exhausted_action),
        file_overrides.and_then(|o| o.budget_exhausted_action),
        env.budget_exhausted_action,
        cli.budget_exhausted_action,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("BUDGET_EXHAUSTED_ACTION"),
    );
    let accounting_scope = layer(
        mode_defaults.accounting_scope,
        None,
        Some(mode_defaults.accounting_scope),
        file_overrides.and_then(|o| o.accounting_scope),
        env.accounting_scope,
        cli.accounting_scope,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ACCOUNTING_SCOPE"),
    );
    let allow_stub_client = layer(
        mode_defaults.allow_stub_client,
        None,
        Some(mode_defaults.allow_stub_client),
        file_overrides.and_then(|o| o.allow_stub_client),
        env.allow_stub_client,
        cli.allow_stub_client,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("ALLOW_STUB_CLIENT"),
    );
    let require_node_auth = layer(
        mode_defaults.require_node_auth,
        None,
        Some(mode_defaults.require_node_auth),
        file_overrides.and_then(|o| o.require_node_auth),
        env.require_node_auth,
        cli.require_node_auth,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("REQUIRE_NODE_AUTH"),
    );
    let config_log_format = layer(
        mode_defaults.config_log_format,
        None,
        Some(mode_defaults.config_log_format),
        file_overrides.and_then(|o| o.config_log_format),
        env.config_log_format,
        cli.config_log_format,
        &topology_source,
        &mode_source,
        &file_source,
        &env_var!("CONFIG_LOG_FORMAT"),
    );

    Ok(ResolvedConfig {
        topology,
        mode,
        connection_mode,
        auth,
        round_timeout_secs,
        min_reputation_score,
        client_registry_ttl,
        quorum,
        selector,
        seed_mode,
        seed_value,
        aggregator,
        robust_byzantine_fraction,
        clip_radius,
        reputation_filter_enabled,
        client_side_privacy_transform,
        privacy_mechanism,
        clip_norm,
        noise_multiplier,
        target_epsilon,
        delta,
        budget_exhausted_action,
        accounting_scope,
        allow_stub_client,
        require_node_auth,
        config_log_format,
    })
}

impl ResolvedConfig {
    /// Every resolved parameter, one log line each, in `format` — this is
    /// what makes resolution explainable "out loud" rather than just
    /// internally consistent. `conflux-server` emits these at startup,
    /// before reaching "ready", so a misconfigured deployment can be
    /// debugged from its own log rather than by reading source: each
    /// line names the parameter, its resolved value, and which of the
    /// six tiers produced it.
    // Sequential pushes read more clearly here than one giant `vec![]`
    // literal spanning ~20 multi-line, heterogeneously-typed `log_line`
    // calls.
    #[allow(clippy::vec_init_then_push)]
    pub fn to_log_lines(&self, format: LogFormat) -> Vec<String> {
        let mut lines = Vec::new();

        lines.push(log_line(
            format,
            "connection_mode",
            LoggedValue::Text(self.connection_mode.value.as_str()),
            &self.connection_mode.source,
        ));
        lines.push(log_line(
            format,
            "auth",
            LoggedValue::Text(self.auth.value.as_str()),
            &self.auth.source,
        ));
        lines.push(log_line(
            format,
            "round_timeout_secs",
            LoggedValue::Number(self.round_timeout_secs.value.to_string()),
            &self.round_timeout_secs.source,
        ));
        lines.push(log_line(
            format,
            "min_reputation_score",
            LoggedValue::Number(self.min_reputation_score.value.to_string()),
            &self.min_reputation_score.source,
        ));
        lines.push(log_line(
            format,
            "client_registry_ttl",
            LoggedValue::Number(self.client_registry_ttl.value.to_string()),
            &self.client_registry_ttl.source,
        ));
        if let Some(quorum) = &self.quorum {
            lines.push(log_line(
                format,
                "quorum",
                LoggedValue::Number(quorum.value.to_string()),
                &quorum.source,
            ));
        }
        lines.push(log_line(
            format,
            "selector",
            LoggedValue::Text(&self.selector.value),
            &self.selector.source,
        ));
        lines.push(log_line(
            format,
            "seed_mode",
            LoggedValue::Text(self.seed_mode.value.as_str()),
            &self.seed_mode.source,
        ));
        lines.push(log_line(
            format,
            "seed_value",
            match self.seed_value.value {
                Some(value) => LoggedValue::Number(value.to_string()),
                None => LoggedValue::Text("n/a"),
            },
            &self.seed_value.source,
        ));
        lines.push(log_line(
            format,
            "aggregator",
            LoggedValue::Text(&self.aggregator.value),
            &self.aggregator.source,
        ));
        lines.push(log_line(
            format,
            "robust_byzantine_fraction",
            LoggedValue::Number(self.robust_byzantine_fraction.value.to_string()),
            &self.robust_byzantine_fraction.source,
        ));
        lines.push(log_line(
            format,
            "clip_radius",
            LoggedValue::Number(self.clip_radius.value.to_string()),
            &self.clip_radius.source,
        ));
        lines.push(log_line(
            format,
            "reputation_filter_enabled",
            LoggedValue::Number(self.reputation_filter_enabled.value.to_string()),
            &self.reputation_filter_enabled.source,
        ));
        lines.push(log_line(
            format,
            "client_side_privacy_transform",
            LoggedValue::Number(self.client_side_privacy_transform.value.to_string()),
            &self.client_side_privacy_transform.source,
        ));
        lines.push(log_line(
            format,
            "privacy_mechanism",
            LoggedValue::Text(&self.privacy_mechanism.value),
            &self.privacy_mechanism.source,
        ));
        lines.push(log_line(
            format,
            "clip_norm",
            LoggedValue::Number(self.clip_norm.value.to_string()),
            &self.clip_norm.source,
        ));
        lines.push(log_line(
            format,
            "noise_multiplier",
            LoggedValue::Number(self.noise_multiplier.value.to_string()),
            &self.noise_multiplier.source,
        ));
        lines.push(log_line(
            format,
            "target_epsilon",
            LoggedValue::Number(self.target_epsilon.value.to_string()),
            &self.target_epsilon.source,
        ));
        lines.push(log_line(
            format,
            "delta",
            LoggedValue::Number(self.delta.value.to_string()),
            &self.delta.source,
        ));
        lines.push(log_line(
            format,
            "budget_exhausted_action",
            LoggedValue::Text(self.budget_exhausted_action.value.as_str()),
            &self.budget_exhausted_action.source,
        ));
        lines.push(log_line(
            format,
            "accounting_scope",
            LoggedValue::Text(self.accounting_scope.value.as_str()),
            &self.accounting_scope.source,
        ));
        lines.push(log_line(
            format,
            "allow_stub_client",
            LoggedValue::Number(self.allow_stub_client.value.to_string()),
            &self.allow_stub_client.source,
        ));
        lines.push(log_line(
            format,
            "require_node_auth",
            LoggedValue::Number(self.require_node_auth.value.to_string()),
            &self.require_node_auth.source,
        ));
        lines.push(log_line(
            format,
            "config_log_format",
            LoggedValue::Text(self.config_log_format.value.as_str()),
            &self.config_log_format.source,
        ));

        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_fallback_wins_with_no_overrides_or_axis_ownership() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 1.0);
        assert_eq!(resolved.clip_norm.source, ConfigSource::BuiltinFallback);
    }

    #[test]
    fn topology_profile_wins_over_builtin() {
        let resolved = resolve(
            Topology::CrossDevice,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.round_timeout_secs.value, 300);
        assert_eq!(
            resolved.round_timeout_secs.source,
            ConfigSource::TopologyProfile("cross_device".to_string())
        );
    }

    #[test]
    fn mode_profile_wins_over_topology_for_mode_owned_params() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Production,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert!(!resolved.allow_stub_client.value);
        assert_eq!(
            resolved.allow_stub_client.source,
            ConfigSource::ModeProfile("production".to_string())
        );
    }

    #[test]
    fn require_node_auth_defaults_off_in_research_and_on_in_production() {
        let research = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();
        assert!(!research.require_node_auth.value);
        assert_eq!(
            research.require_node_auth.source,
            ConfigSource::ModeProfile("research".to_string())
        );

        let production = resolve(
            Topology::CrossSilo,
            Mode::Production,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();
        assert!(production.require_node_auth.value);
        assert_eq!(
            production.require_node_auth.source,
            ConfigSource::ModeProfile("production".to_string())
        );
    }

    #[test]
    fn require_node_auth_explicit_override_wins_over_mode_default() {
        let cli_overrides = Overrides {
            require_node_auth: Some(true),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert!(resolved.require_node_auth.value);
        assert_eq!(resolved.require_node_auth.source, ConfigSource::Cli);
    }

    #[test]
    fn robust_byzantine_fraction_defaults_to_builtin_fallback() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.robust_byzantine_fraction.value, 0.2);
        assert_eq!(
            resolved.robust_byzantine_fraction.source,
            ConfigSource::BuiltinFallback
        );
    }

    #[test]
    fn robust_byzantine_fraction_explicit_override_wins() {
        let cli_overrides = Overrides {
            robust_byzantine_fraction: Some(0.33),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.robust_byzantine_fraction.value, 0.33);
        assert_eq!(resolved.robust_byzantine_fraction.source, ConfigSource::Cli);
    }

    #[test]
    fn client_side_privacy_transform_defaults_off_everywhere() {
        // Opt-in, like every other privacy/security posture here. No
        // topology or mode turns it on for you: transforming an update
        // twice is a deployment's deliberate choice about its own trust
        // boundaries, not something a profile should assume.
        for topology in [
            Topology::CrossSilo,
            Topology::CrossDevice,
            Topology::Crowdsource,
            Topology::Edge,
        ] {
            for mode in [Mode::Research, Mode::Production] {
                let resolved = resolve(
                    topology,
                    mode,
                    None,
                    &Overrides::default(),
                    &Overrides::default(),
                )
                .unwrap();

                assert!(!resolved.client_side_privacy_transform.value);
                assert_eq!(
                    resolved.client_side_privacy_transform.source,
                    ConfigSource::BuiltinFallback
                );
            }
        }
    }

    #[test]
    fn client_side_privacy_transform_explicit_override_wins() {
        let cli_overrides = Overrides {
            client_side_privacy_transform: Some(true),
            ..Default::default()
        };
        let resolved = resolve(
            Topology::Crowdsource,
            Mode::Production,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert!(resolved.client_side_privacy_transform.value);
        assert_eq!(
            resolved.client_side_privacy_transform.source,
            ConfigSource::Cli
        );
    }

    #[test]
    fn clip_radius_defaults_to_builtin_fallback() {
        // No topology or mode owns an opinion on it — the right radius
        // depends on the model's weight scale, not the deployment shape
        // — so every topology/mode pair must land on the same fallback.
        for topology in [
            Topology::CrossSilo,
            Topology::CrossDevice,
            Topology::Crowdsource,
            Topology::Edge,
        ] {
            for mode in [Mode::Research, Mode::Production] {
                let resolved = resolve(
                    topology,
                    mode,
                    None,
                    &Overrides::default(),
                    &Overrides::default(),
                )
                .unwrap();

                assert_eq!(resolved.clip_radius.value, 1.0);
                assert_eq!(resolved.clip_radius.source, ConfigSource::BuiltinFallback);
            }
        }
    }

    #[test]
    fn clip_radius_explicit_override_wins() {
        let cli_overrides = Overrides {
            clip_radius: Some(2.5),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.clip_radius.value, 2.5);
        assert_eq!(resolved.clip_radius.source, ConfigSource::Cli);
    }

    #[test]
    fn reputation_filter_enabled_defaults_off() {
        // Every aggregator's default behavior should match its cited
        // paper, with zero framework-imposed interference unless a
        // deployment explicitly opts in.
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert!(!resolved.reputation_filter_enabled.value);
        assert_eq!(
            resolved.reputation_filter_enabled.source,
            ConfigSource::BuiltinFallback
        );
    }

    #[test]
    fn reputation_filter_enabled_explicit_override_wins() {
        let cli_overrides = Overrides {
            reputation_filter_enabled: Some(true),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert!(resolved.reputation_filter_enabled.value);
        assert_eq!(resolved.reputation_filter_enabled.source, ConfigSource::Cli);
    }

    #[test]
    fn file_wins_over_topology_and_mode() {
        let file_overrides = Overrides {
            round_timeout_secs: Some(120),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossDevice,
            Mode::Research,
            Some(("experiment.toml", &file_overrides)),
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.round_timeout_secs.value, 120);
        assert_eq!(
            resolved.round_timeout_secs.source,
            ConfigSource::ExperimentFile("experiment.toml".to_string())
        );
    }

    #[test]
    fn env_wins_over_file() {
        let file_overrides = Overrides {
            clip_norm: Some(2.0),
            ..Default::default()
        };
        let env_overrides = Overrides {
            clip_norm: Some(3.0),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            Some(("experiment.toml", &file_overrides)),
            &env_overrides,
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 3.0);
        assert_eq!(
            resolved.clip_norm.source,
            ConfigSource::EnvVar("CONFLUX_CLIP_NORM".to_string())
        );
    }

    #[test]
    fn cli_wins_over_everything() {
        let env_overrides = Overrides {
            clip_norm: Some(3.0),
            ..Default::default()
        };
        let cli_overrides = Overrides {
            clip_norm: Some(4.0),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &env_overrides,
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.clip_norm.value, 4.0);
        assert_eq!(resolved.clip_norm.source, ConfigSource::Cli);
    }

    #[test]
    fn quorum_has_no_builtin_fallback() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert!(resolved.quorum.is_none());
    }

    #[test]
    fn quorum_resolves_when_overridden() {
        let cli_overrides = Overrides {
            quorum: Some(5),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .unwrap();

        assert_eq!(resolved.quorum.unwrap().value, 5);
    }

    #[test]
    fn seed_value_is_none_in_production_by_default() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Production,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        assert_eq!(resolved.seed_value.value, None);
        assert_eq!(
            resolved.seed_value.source,
            ConfigSource::ModeProfile("production".to_string())
        );
    }

    #[test]
    fn per_client_accounting_resolves_successfully() {
        // AccountingScope::PerClient is a real, implemented scope —
        // resolves the same way as Global, with no special-case failure.
        let cli_overrides = Overrides {
            accounting_scope: Some(AccountingScope::PerClient),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .expect("PerClient must resolve successfully");

        assert_eq!(resolved.accounting_scope.value, AccountingScope::PerClient);
    }

    #[test]
    fn json_log_line_matches_spec_example_shape() {
        let resolved = resolve(
            Topology::CrossDevice,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        let lines = resolved.to_log_lines(LogFormat::Json);
        let round_timeout_line = lines
            .iter()
            .find(|line| line.contains("\"param\":\"round_timeout_secs\""))
            .expect("round_timeout_secs line present");

        assert_eq!(
            round_timeout_line,
            "{\"param\":\"round_timeout_secs\",\"value\":300,\"source\":\"topology_profile\",\"profile\":\"cross_device\"}"
        );
    }

    #[test]
    fn text_log_line_matches_spec_example_shape() {
        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &Overrides::default(),
        )
        .unwrap();

        let lines = resolved.to_log_lines(LogFormat::Text);
        let clip_norm_line = lines
            .iter()
            .find(|line| line.starts_with("[config] clip_norm"))
            .expect("clip_norm line present");

        assert!(clip_norm_line.contains("= 1"));
        assert!(clip_norm_line.contains("(source: built-in fallback)"));
    }

    #[test]
    fn empty_string_aggregator_override_resolves_without_error() {
        // conflux-config resolves whatever String it's given for
        // `aggregator`/`selector` — it doesn't validate the name against
        // the strategy registry itself. That check happens downstream,
        // in whichever crate (conflux-core, conflux-selector) actually
        // constructs the implementation by calling `registry::lookup`.
        // An empty string is accepted here, and only fails later, at
        // construction time — worth confirming explicitly, since it's a
        // real boundary a reader could otherwise assume conflux-config
        // enforces.
        let cli_overrides = Overrides {
            aggregator: Some(String::new()),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .expect("resolve() does not validate strategy names");

        assert_eq!(resolved.aggregator.value, "");
        assert_eq!(resolved.aggregator.source, ConfigSource::Cli);
    }

    #[test]
    fn out_of_range_numeric_overrides_are_not_rejected_by_resolve() {
        // Same boundary as the test above, for numeric parameters:
        // resolve() carries whatever value it's given through to
        // ResolvedConfig without range-checking it. A negative
        // target_epsilon or a huge clip_norm is nonsensical for the
        // privacy/DP math that consumes these values downstream
        // (conflux-privacy), but that validation — if it exists — lives
        // there, not here. Documenting the absence, not asserting it's
        // correct.
        let cli_overrides = Overrides {
            target_epsilon: Some(-1.0),
            clip_norm: Some(f32::MAX),
            ..Default::default()
        };

        let resolved = resolve(
            Topology::CrossSilo,
            Mode::Research,
            None,
            &Overrides::default(),
            &cli_overrides,
        )
        .expect("resolve() does not range-check numeric overrides");

        assert_eq!(resolved.target_epsilon.value, -1.0);
        assert_eq!(resolved.clip_norm.value, f32::MAX);
    }
}
