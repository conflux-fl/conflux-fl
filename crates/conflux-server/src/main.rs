//! Server binary — integrates the library crates into the round pipeline.
//!
//! See `docs/spec/conflux-spec-v1.md` §8, §10 (Phase 5). CLI/experiment-file
//! parsing into `conflux-config::Overrides` isn't built yet (spec §11 Open
//! Item 2) — topology/mode are picked from `CONFLUX_TOPOLOGY`/`CONFLUX_MODE`
//! env vars for now.
//!
//! Backend selection (Phase 8a) is env-var driven too, deliberately kept
//! separate from `conflux-config`'s `Overrides` — see
//! `docs/phases/phase-8a-backend-selection.md`'s scope note: a Redis URL is
//! a deployment detail, not an experiment-tuning parameter.
//!
//! Node auth (Phase 8b/8c) needs no separate wiring here: `require_node_auth`
//! is a regular `conflux-config` parameter (already covered by the
//! provenance-log loop below), and `AppState::connect` derives the
//! allow-list backend from `CONFLUX_REGISTRY_BACKEND` itself — see
//! `docs/phases/phase-8c-node-auth-enforcement.md`'s scope note on why
//! that's one fewer env var rather than a fully independent backend axis.
//!
//! `overrides_from_env` (below) closes part of the gap flagged in
//! `docs/STATUS.md`'s "Next" section after Phase 11c's manual
//! verification needed a throwaway example binary to select a
//! non-default aggregator: `CONFLUX_AGGREGATOR`/`CONFLUX_SELECTOR`/
//! `CONFLUX_PRIVACY_MECHANISM`/`CONFLUX_ROBUST_BYZANTINE_FRACTION`, plus
//! `CONFLUX_QUORUM`/`CONFLUX_ROUND_TIMEOUT_SECS`/`CONFLUX_CLIP_NORM`/
//! `CONFLUX_NOISE_MULTIPLIER`/`CONFLUX_MIN_REPUTATION_SCORE` (needed to
//! run `docs/E2E_TESTING.md`'s harness without code changes — the last
//! one specifically to isolate `robust`-family aggregation defenses from
//! `conflux-reputation`'s own, separately-vulnerable filtering stage;
//! see that doc's "A real finding" section). A focused, demo-motivated
//! expansion, not full config-file parsing — spec §11 Open Item 2 stays
//! open for the remaining `Overrides` fields.
//!
//! `CONFLUX_GRPC_ADDR`/`CONFLUX_HTTP_ADDR` (below) close a gap
//! `docs/WEB_APP_INTEGRATION.md` surfaced: both listeners were hardcoded
//! to `127.0.0.1`, which is unreachable from a separate container (e.g. a
//! FastAPI/Django backend calling the HTTP admin API from its own
//! container) unless it shares this process's network namespace. Defaults
//! stay loopback-only — the admin API has no auth of its own, so binding
//! wider is an explicit opt-in, not a new default.
//!
//! `CONFLUX_REPUTATION_FILTER_ENABLED` (Phase 13): reputation filtering
//! is opt-in, defaulting to `false` — `conflux-reputation`'s
//! `CosineScorer`, applied unconditionally in front of every aggregator,
//! was itself the bug the "real finding" above documents: no cited paper
//! (Krum, Trimmed Mean, Median, ...) asks for an extra uncited filter
//! ahead of it. See `docs/phases/phase-13-reputation-reference-fix.md`.
//! `CONFLUX_MIN_REPUTATION_SCORE` still controls the threshold used
//! *when* this is explicitly turned on.

use conflux_net::jwt::JwtKeyMaterial;
use std::net::SocketAddr;
use std::sync::Arc;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::FlTransportService;
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_server::{
    AccountingBackend, AppState, BackendSelection, RegistryBackend, StoreBackend, TlsMaterial,
    resolve_server_tls, run_round, validate_jwt_startup,
};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let topology = match std::env::var("CONFLUX_TOPOLOGY").as_deref() {
        Ok("cross_silo") => Topology::CrossSilo,
        Ok("crowdsource") => Topology::Crowdsource,
        Ok("edge") => Topology::Edge,
        _ => Topology::CrossDevice,
    };
    let mode = match std::env::var("CONFLUX_MODE").as_deref() {
        Ok("production") => Mode::Production,
        _ => Mode::Research,
    };

    // Phase 20: an optional experiment-level config file. Unset behaves
    // exactly as before — `None` into the tier that has always been
    // there. Set, it is a hard failure if unreadable: an operator who
    // named a config file meant it, and silently continuing with
    // defaults would produce a run whose logged provenance is correct
    // and whose configuration is not what anyone asked for.
    let experiment_file = std::env::var("CONFLUX_EXPERIMENT_CONFIG_PATH").ok();
    let file_overrides = experiment_file.as_ref().map(|path| {
        conflux_config::load_experiment_file(std::path::Path::new(path))
            .unwrap_or_else(|e| panic!("{e}"))
    });
    let file_tier = match (&experiment_file, &file_overrides) {
        (Some(path), Some(overrides)) => Some((path.as_str(), overrides)),
        _ => None,
    };

    let config = conflux_config::resolve(
        topology,
        mode,
        file_tier,
        &overrides_from_env(),
        &Overrides::default(),
    )
    .expect("config resolution failed");

    // ADR 0007: every resolved parameter is logged before the server is
    // "ready".
    for line in config.to_log_lines(config.config_log_format.value) {
        println!("{line}");
    }

    // Phase 9a: makes the just-logged `auth` value real — `mode =
    // production` with `auth = mtls` and no TLS material refuses to
    // start here (`resolve_server_tls`'s own fail-fast), rather than
    // silently binding a plaintext gRPC server for a topology whose
    // profile says it should require mTLS.
    let tls_material = tls_material_from_env();
    let tls_config =
        resolve_server_tls(mode, config.auth.value, tls_material).expect("auth enforcement failed");
    if tls_config.is_none() && config.auth.value == conflux_config::AuthMode::Mtls {
        tracing::warn!(
            "auth resolved to mtls but no TLS material was configured; falling back to \
             plaintext (research mode only — production would have refused to start)"
        );
    }

    // `CONFLUX_INITIAL_WEIGHTS_DIM`: the real model this deployment trains
    // dictates this, not Conflux (ADR 0004 — a flat f32 vector is all
    // Conflux ever sees) — e.g. `docs/E2E_TESTING.md`'s harness sets this
    // to its logistic-regression model's actual parameter count. Every
    // client's submitted weights must match this dimension or
    // `AggregatorError::MismatchedLength` rejects the round.
    let initial_weights_dim: usize = std::env::var("CONFLUX_INITIAL_WEIGHTS_DIM")
        .ok()
        .map(|v| {
            v.parse()
                .expect("CONFLUX_INITIAL_WEIGHTS_DIM must be a positive integer")
        })
        .unwrap_or(4);
    // Phase 16: the `auth = jwt` counterpart to the mTLS check above.
    // Loaded and validated *before* binding, so a production JWT
    // deployment with no key to verify against never starts — the same
    // fail-fast discipline, for the other three topologies' default
    // auth mode.
    let jwt_key = jwt_key_from_env();
    validate_jwt_startup(mode, config.auth.value, jwt_key.as_ref())
        .expect("auth enforcement failed");
    match (&jwt_key, config.auth.value) {
        (Some(key), conflux_config::AuthMode::Jwt) => {
            tracing::info!(
                algorithm = key.algorithm(),
                "auth = jwt; every register() will be verified against the configured public key"
            );
        }
        (None, conflux_config::AuthMode::Jwt) => {
            tracing::warn!(
                "auth resolved to jwt but no CONFLUX_JWT_PUBLIC_KEY_PATH was configured; \
                 auth_token will not be verified (research mode only — production would \
                 have refused to start)"
            );
        }
        // A key supplied under `auth = mtls` is configuration that does
        // nothing. Said out loud rather than ignored (ADR 0007): the
        // operator plainly intended it to be used.
        (Some(_), _) => tracing::warn!(
            "CONFLUX_JWT_PUBLIC_KEY_PATH is set but auth resolved to mtls; \
             no token will be verified"
        ),
        (None, _) => {}
    }

    let initial_weights = vec![0.0f32; initial_weights_dim];
    let backends = backend_selection_from_env();
    let state = Arc::new(
        AppState::connect(config, mode, initial_weights, backends)
            .await
            .expect("backend connection failed")
            .with_jwt_key(jwt_key),
    );

    let grpc_addr: SocketAddr = std::env::var("CONFLUX_GRPC_ADDR")
        .ok()
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("CONFLUX_GRPC_ADDR={v:?} is not a valid socket address"))
        })
        .unwrap_or_else(|| "127.0.0.1:50051".parse().unwrap());
    let http_addr: SocketAddr = std::env::var("CONFLUX_HTTP_ADDR")
        .ok()
        .map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("CONFLUX_HTTP_ADDR={v:?} is not a valid socket address"))
        })
        .unwrap_or_else(|| "127.0.0.1:8080".parse().unwrap());

    let grpc_state = Arc::clone(&state);
    let grpc = tokio::spawn(async move {
        let mut builder = tonic::transport::Server::builder();
        if let Some(tls_config) = tls_config {
            builder = builder.tls_config(tls_config).expect("invalid TLS config");
        }
        builder
            .add_service(FlTransportServer::new(FlTransportService::new(grpc_state)))
            .serve(grpc_addr)
            .await
            .expect("grpc server failed");
    });

    let http_state = Arc::clone(&state);
    let http = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr)
            .await
            .expect("http bind failed");
        axum::serve(listener, conflux_server::router(http_state))
            .await
            .expect("http server failed");
    });

    let round_state = Arc::clone(&state);
    let rounds = tokio::spawn(async move {
        loop {
            match run_round(&round_state).await {
                Ok(summary) => tracing::info!(?summary, "round completed"),
                // No client has submitted yet (often: none have registered
                // yet) — retryable, not fatal. Every other error (a
                // genuinely exhausted privacy budget, a store/registry
                // failure) stops the loop.
                Err(conflux_server::ServerError::Aggregator(
                    conflux_core::AggregatorError::EmptyBatch,
                )) => {
                    tracing::info!("no submissions yet this round; retrying shortly");
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                Err(e) => {
                    tracing::error!(error = %e, "round loop stopped");
                    break;
                }
            }
        }
    });

    let _ = tokio::join!(grpc, http, rounds);
}

/// Reads a handful of `conflux-config` `Overrides` fields from their own
/// `CONFLUX_*` env vars — see this module's doc comment for exactly
/// which fields and why these specifically. Every field is optional;
/// missing means "let the topology/mode profile or builtin fallback
/// decide," same as every other tier `conflux_config::resolve` layers.
fn overrides_from_env() -> Overrides {
    fn var<T: std::str::FromStr>(name: &str) -> Option<T> {
        std::env::var(name).ok().map(|v| {
            v.parse()
                .unwrap_or_else(|_| panic!("{name}={v:?} is not a valid value"))
        })
    }

    Overrides {
        aggregator: std::env::var("CONFLUX_AGGREGATOR").ok(),
        selector: std::env::var("CONFLUX_SELECTOR").ok(),
        privacy_mechanism: std::env::var("CONFLUX_PRIVACY_MECHANISM").ok(),
        robust_byzantine_fraction: var("CONFLUX_ROBUST_BYZANTINE_FRACTION"),
        clip_radius: var("CONFLUX_CLIP_RADIUS"),
        min_reputation_score: var("CONFLUX_MIN_REPUTATION_SCORE"),
        reputation_filter_enabled: var("CONFLUX_REPUTATION_FILTER_ENABLED"),
        quorum: var("CONFLUX_QUORUM"),
        round_timeout_secs: var("CONFLUX_ROUND_TIMEOUT_SECS"),
        clip_norm: var("CONFLUX_CLIP_NORM"),
        noise_multiplier: var("CONFLUX_NOISE_MULTIPLIER"),
        ..Default::default()
    }
}

/// Reads `CONFLUX_REGISTRY_BACKEND`/`CONFLUX_REDIS_URL`,
/// `CONFLUX_STORE_BACKEND`/`CONFLUX_POSTGRES_URL`/`CONFLUX_S3_*`, and
/// `CONFLUX_ACCOUNTING_PERSISTENCE` (which reuses `CONFLUX_POSTGRES_URL`)
/// into a `BackendSelection`. Missing/unset means "memory"/"disabled" —
/// `AppState::connect`'s own `validate_production_backends` is what turns
/// that into a startup failure when `mode = production`, so this function
/// doesn't need to know about `mode` at all.
fn backend_selection_from_env() -> BackendSelection {
    let registry = match std::env::var("CONFLUX_REGISTRY_BACKEND").as_deref() {
        Ok("redis") => RegistryBackend::Redis {
            url: std::env::var("CONFLUX_REDIS_URL")
                .expect("CONFLUX_REGISTRY_BACKEND=redis requires CONFLUX_REDIS_URL"),
        },
        _ => RegistryBackend::Memory,
    };

    let store = match std::env::var("CONFLUX_STORE_BACKEND").as_deref() {
        Ok("postgres") => StoreBackend::Postgres {
            url: std::env::var("CONFLUX_POSTGRES_URL")
                .expect("CONFLUX_STORE_BACKEND=postgres requires CONFLUX_POSTGRES_URL"),
        },
        Ok("s3") => StoreBackend::S3 {
            endpoint: std::env::var("CONFLUX_S3_ENDPOINT")
                .expect("CONFLUX_STORE_BACKEND=s3 requires CONFLUX_S3_ENDPOINT"),
            bucket: std::env::var("CONFLUX_S3_BUCKET")
                .expect("CONFLUX_STORE_BACKEND=s3 requires CONFLUX_S3_BUCKET"),
            access_key: std::env::var("CONFLUX_S3_ACCESS_KEY")
                .expect("CONFLUX_STORE_BACKEND=s3 requires CONFLUX_S3_ACCESS_KEY"),
            secret_key: std::env::var("CONFLUX_S3_SECRET_KEY")
                .expect("CONFLUX_STORE_BACKEND=s3 requires CONFLUX_S3_SECRET_KEY"),
        },
        _ => StoreBackend::Memory,
    };

    let accounting = match std::env::var("CONFLUX_ACCOUNTING_PERSISTENCE").as_deref() {
        Ok("true") => AccountingBackend::Postgres {
            url: std::env::var("CONFLUX_POSTGRES_URL")
                .expect("CONFLUX_ACCOUNTING_PERSISTENCE=true requires CONFLUX_POSTGRES_URL"),
        },
        _ => AccountingBackend::Disabled,
    };

    BackendSelection {
        registry,
        store,
        accounting,
    }
}

/// Reads `CONFLUX_TLS_CERT_PATH`/`CONFLUX_TLS_KEY_PATH`/
/// `CONFLUX_TLS_CLIENT_CA_PATH` (PEM file paths) into a `TlsMaterial`.
/// `None` when any of the three is unset — `resolve_server_tls` is what
/// turns that into a startup failure when it matters (`auth = mtls` and
/// `mode = production`), so this function doesn't need to know either.
fn tls_material_from_env() -> Option<TlsMaterial> {
    let cert_path = std::env::var("CONFLUX_TLS_CERT_PATH").ok()?;
    let key_path = std::env::var("CONFLUX_TLS_KEY_PATH").ok()?;
    let client_ca_path = std::env::var("CONFLUX_TLS_CLIENT_CA_PATH").ok()?;

    Some(TlsMaterial {
        cert_pem: std::fs::read(&cert_path)
            .unwrap_or_else(|e| panic!("failed to read CONFLUX_TLS_CERT_PATH ({cert_path}): {e}")),
        key_pem: std::fs::read(&key_path)
            .unwrap_or_else(|e| panic!("failed to read CONFLUX_TLS_KEY_PATH ({key_path}): {e}")),
        client_ca_pem: std::fs::read(&client_ca_path).unwrap_or_else(|e| {
            panic!("failed to read CONFLUX_TLS_CLIENT_CA_PATH ({client_ca_path}): {e}")
        }),
    })
}

/// Reads `CONFLUX_JWT_PUBLIC_KEY_PATH` (a PEM public key) into
/// `JwtKeyMaterial`. `None` when unset — `validate_jwt_startup` is what
/// turns that into a startup failure when it matters (`auth = jwt` and
/// `mode = production`), the same division of labor
/// `tls_material_from_env` has with `resolve_server_tls`.
///
/// A path that is set but unreadable, or readable but not a usable key,
/// panics here rather than degrading to `None`: an operator who named a
/// key file meant it, and silently continuing unauthenticated is the one
/// outcome nobody wants from a misconfigured auth setting.
fn jwt_key_from_env() -> Option<JwtKeyMaterial> {
    let path = std::env::var("CONFLUX_JWT_PUBLIC_KEY_PATH").ok()?;
    let pem = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("failed to read CONFLUX_JWT_PUBLIC_KEY_PATH ({path}): {e}"));
    Some(
        JwtKeyMaterial::from_public_key_pem(&pem)
            .unwrap_or_else(|e| panic!("CONFLUX_JWT_PUBLIC_KEY_PATH ({path}) is unusable: {e}")),
    )
}
