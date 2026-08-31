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

    // ADR 0007's "say so, out loud" applied to a default that is
    // actively dangerous. `clip_radius` has a builtin fallback so the
    // config layer has something to resolve, but there is no value that
    // is right for an unknown model — and the placeholder measured
    // *worse than no defense at all* on a real one (§5.13). An operator
    // who selected this aggregator and never set the radius has almost
    // certainly not made a choice; say so before serving a round.
    if config.aggregator.value == "centered_clipping"
        && config.clip_radius.source == conflux_config::ConfigSource::BuiltinFallback
    {
        tracing::warn!(
            clip_radius = config.clip_radius.value,
            "aggregator = centered_clipping with an untuned clip_radius. This is a \
             placeholder, not a default: on a real 50,890-parameter model it scored below \
             undefended fedavg. Tune CONFLUX_CLIP_RADIUS to your model's weight scale, or \
             use a selection-based robust aggregator instead."
        );
    }

    let initial_weights = vec![0.0f32; initial_weights_dim];
    let backends = backend_selection_from_env();
    let mut state = AppState::connect(config, mode, initial_weights, backends)
        .await
        .expect("backend connection failed")
        .with_jwt_key(jwt_key);

    // ADR 0011: connect to the trusted-reference sidecar, but only if the
    // configured aggregator actually needs one. Asking the aggregator
    // rather than checking whether the env var is set keeps the two
    // failure directions symmetric — a sidecar configured for `fedavg` is
    // ignored, and `fltrust` without a sidecar refuses to start.
    if state.aggregator.requires_trusted_reference() {
        state = connect_trusted_reference(state, initial_weights_dim).await;
    } else if std::env::var("CONFLUX_TRUSTED_REFERENCE_ADDR").is_ok() {
        tracing::warn!(
            aggregator = %state.config.aggregator.value,
            "CONFLUX_TRUSTED_REFERENCE_ADDR is set, but this aggregator does not use a \
             trusted reference — no sidecar connection will be opened"
        );
    }

    let state = Arc::new(state);

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

    // The HTTP admin surface's own gate, beside the gRPC ones above.
    // `/admin/allowlist` decides who may participate, so an
    // unauthenticated admin API bound anywhere reachable would undo the
    // authentication on the gRPC port entirely.
    let admin_token = std::env::var("CONFLUX_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .map(conflux_server::AdminToken::new);
    conflux_server::validate_admin_binding(http_addr, admin_token.as_ref())
        .expect("admin API binding refused");
    if admin_token.is_some() {
        tracing::info!("HTTP admin API requires a bearer token (CONFLUX_ADMIN_TOKEN)");
    } else {
        tracing::warn!(
            %http_addr,
            "HTTP admin API is unauthenticated — permitted only because it is bound to \
             loopback. Set CONFLUX_ADMIN_TOKEN before binding it anywhere else."
        );
    }

    // One `watch` channel, three consumers: the two servers stop accepting
    // work, and the round loop finishes the round it is in and then exits
    // (Tier 5, H3). `watch` rather than `broadcast` because the value is a
    // latch — a late subscriber must still see that shutdown was requested,
    // which a missed broadcast message would not give it.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _ = shutdown_tx.send(true);
    });

    let grpc_state = Arc::clone(&state);
    // Read before the spawn: `state` is moved into the round loop below,
    // and the bound is a plain `u64`.
    let max_update_bytes = state.config.max_update_bytes.value;
    let mut grpc_shutdown = shutdown_rx.clone();
    let grpc = tokio::spawn(async move {
        let mut builder = tonic::transport::Server::builder();
        if let Some(tls_config) = tls_config {
            builder = builder.tls_config(tls_config).expect("invalid TLS config");
        }
        builder
            .add_service(FlTransportServer::new(
                FlTransportService::new(grpc_state).with_max_update_bytes(max_update_bytes),
            ))
            .serve_with_shutdown(grpc_addr, async move {
                // `changed()` waits for the next send, so a shutdown that
                // fired before this task got scheduled would be missed —
                // hence the initial `borrow()` check.
                if *grpc_shutdown.borrow() {
                    return;
                }
                let _ = grpc_shutdown.changed().await;
            })
            .await
            .expect("grpc server failed");
        tracing::info!("grpc server stopped accepting connections");
    });

    let http_state = Arc::clone(&state);
    let http_token = admin_token.clone();
    let mut http_shutdown = shutdown_rx.clone();
    let http = tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(http_addr)
            .await
            .expect("http bind failed");
        axum::serve(listener, conflux_server::router(http_state, http_token))
            .with_graceful_shutdown(async move {
                if *http_shutdown.borrow() {
                    return;
                }
                let _ = http_shutdown.changed().await;
            })
            .await
            .expect("http server failed");
        tracing::info!("http server stopped accepting connections");
    });

    let round_state = Arc::clone(&state);
    let health = Arc::clone(&state.round_loop_health);
    let mut round_shutdown = shutdown_rx.clone();
    let rounds = tokio::spawn(async move {
        loop {
            // Checked between rounds, never during one. `run_round` is
            // awaited as a unit below, so a shutdown that arrives mid-round
            // waits for that round to finish rather than abandoning
            // buffered submissions and a half-written checkpoint (Tier 5,
            // H3).
            if *round_shutdown.borrow() {
                tracing::info!("shutdown requested; round loop exiting between rounds");
                health.record_stopped(None);
                break;
            }

            match run_round(&round_state).await {
                Ok(summary) => {
                    tracing::info!(?summary, "round completed");
                    health.record_success(summary.round);
                }
                // Tier 5 (H2). This used to `break` on everything but
                // `EmptyBatch`, so one Redis reconnect ended the
                // experiment permanently while the process stayed up.
                // `is_transient` draws the line — see `ServerError` for
                // why it falls where it does.
                Err(e) if e.is_transient() => {
                    let failures = health.record_transient_failure(&e.to_string());
                    let delay = conflux_server::backoff_secs(failures);
                    // `EmptyBatch` is the ordinary "nobody has registered
                    // yet" case and would be alarming at warn level every
                    // two seconds on a freshly-started server.
                    if matches!(
                        e,
                        conflux_server::ServerError::Aggregator(
                            conflux_core::AggregatorError::EmptyBatch
                        )
                    ) {
                        tracing::info!(
                            retry_in_secs = delay,
                            "no submissions yet this round; retrying"
                        );
                    } else {
                        tracing::warn!(
                            error = %e,
                            consecutive_failures = failures,
                            retry_in_secs = delay,
                            "round failed with a retryable error; backing off"
                        );
                    }
                    // Racing the sleep against shutdown, so Ctrl-C during a
                    // 60-second backoff doesn't wait out the backoff.
                    tokio::select! {
                        _ = tokio::time::sleep(std::time::Duration::from_secs(delay)) => {}
                        _ = round_shutdown.changed() => {}
                    }
                }
                Err(e) => {
                    // Fatal: an exhausted privacy budget. Stopping is the
                    // specified behavior here, not a failure to handle
                    // something — but `/health` now reports it.
                    tracing::error!(error = %e, "round loop stopped: unrecoverable");
                    health.record_stopped(Some(&e.to_string()));
                    break;
                }
            }
        }
    });

    let _ = tokio::join!(grpc, http, rounds);
    tracing::info!("shutdown complete");
}

/// Resolves when the process is asked to stop: Ctrl-C on any platform, or
/// `SIGTERM` on Unix.
///
/// `SIGTERM` is the one that matters in production and the one that was
/// missing (Tier 5, H3) — it is what `docker stop`, a Kubernetes eviction,
/// and systemd all send first, with `SIGKILL` following after a grace
/// period. Without a handler the default disposition terminates the process
/// immediately, so the grace period was being spent doing nothing and the
/// round in flight was lost either way.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            // Failing to install the handler must not take the server
            // down — it just means SIGTERM keeps its default disposition,
            // which is what happened before this function existed.
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl-C; shutting down"),
        _ = terminate => tracing::info!("received SIGTERM; shutting down"),
    }
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
        server_learning_rate: var("CONFLUX_SERVER_LEARNING_RATE"),
        server_tau: var("CONFLUX_SERVER_TAU"),
        min_reputation_score: var("CONFLUX_MIN_REPUTATION_SCORE"),
        reputation_filter_enabled: var("CONFLUX_REPUTATION_FILTER_ENABLED"),
        quorum: var("CONFLUX_QUORUM"),
        max_update_bytes: var("CONFLUX_MAX_UPDATE_BYTES"),
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

/// Connects to the trusted-reference sidecar and verifies it can serve
/// the configured aggregator (ADR 0011).
///
/// Panics on every failure, deliberately, and in the same register as
/// `validate_production_backends` and `allow_stub_client`: a server that
/// starts without the signal its aggregator is defined in terms of would
/// accept clients, run rounds, and write checkpoints that look healthy
/// while the defense it was configured for is simply absent. Failing to
/// start is the correct response.
///
/// The `Describe` handshake is why this is a startup check rather than a
/// round-one discovery: a deployer who points `fltrust` at a
/// scoring-only sidecar finds out before any client has connected.
async fn connect_trusted_reference(state: AppState, initial_weights_dim: usize) -> AppState {
    let addr = std::env::var("CONFLUX_TRUSTED_REFERENCE_ADDR").unwrap_or_else(|_| {
        panic!(
            "aggregator = {:?} requires a trusted-reference sidecar (ADR 0011), but \
             CONFLUX_TRUSTED_REFERENCE_ADDR is not set. Start one — \
             `cargo run -p conflux-trusted-reference` — or choose an aggregator that \
             scores from the batch alone.",
            state.config.aggregator.value
        )
    });

    let mut transport = conflux_net::TrustedReferenceTransport::connect(addr.clone())
        .await
        .unwrap_or_else(|e| panic!("could not reach the trusted-reference sidecar at {addr}: {e}"));

    let capabilities = transport
        .describe()
        .await
        .unwrap_or_else(|e| panic!("the sidecar at {addr} did not answer Describe: {e}"));

    if !capabilities.supports_reference_update {
        panic!(
            "the sidecar at {addr} ({}) does not implement reference updates, so it cannot \
             serve aggregator {:?}",
            capabilities.description, state.config.aggregator.value
        );
    }

    // The dimension check is advisory rather than fatal: a sidecar that
    // builds its model lazily legitimately answers `None`, and refusing
    // to start on "did not say" would rule out a valid implementation.
    // A mismatch it *did* state, though, is a misconfiguration worth
    // stopping for — the alternative is discovering it as a length error
    // in round one.
    let experiment_dim = initial_weights_dim;
    match capabilities.model_dim {
        Some(dim) if dim as usize != experiment_dim => {
            panic!(
                "the sidecar at {addr} serves a {dim}-weight model, but this experiment's \
                 model has {experiment_dim} weights"
            );
        }
        Some(_) => {}
        None => tracing::info!(
            %addr,
            "the sidecar did not state a model dimension; a mismatch will surface at \
             the first round instead of now"
        ),
    }

    tracing::info!(
        %addr,
        model = %capabilities.description,
        supports_scoring = capabilities.supports_scoring,
        "connected to the trusted-reference sidecar"
    );

    state.with_trusted_reference(transport)
}
