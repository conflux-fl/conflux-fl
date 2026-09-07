//! Running a server: from the environment to a listening deployment,
//! and the shutdown that ends it.
//!
//! This lives in the library rather than the binary so that anything
//! wanting to *run* a Conflux server runs the same code the server
//! binary runs — `cflux server start` is the second caller, and a CLI
//! that reimplemented startup would be a second server with its own
//! bugs. The binary is now a shell: initialize tracing, call
//! [`run_from_env`], report what came back.
//!
//! Every failure is a [`ServeError`] rather than a panic. The binary
//! turns one into a message and a non-zero exit, which is what a panic
//! did; a library that panicked would take that choice away from every
//! other caller.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use conflux_config::{AuthMode, ConfigSource, Overrides, ProfileAxis};
use conflux_net::FlTransportService;
use conflux_proto::fl_transport_server::FlTransportServer;

use crate::{AdminToken, AppState, resolve_server_tls, run_round, validate_jwt_startup};

/// Why a server could not start, or could not finish cleanly.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// A topology or mode name matched neither a builtin nor a file.
    #[error("{0}")]
    Profile(#[from] conflux_config::ProfileError),
    /// An experiment config file could not be read or parsed.
    #[error("{0}")]
    ConfigFile(#[from] conflux_config::ConfigFileError),
    /// A `CONFLUX_*` parameter variable is set but malformed.
    #[error("{0}")]
    Overrides(#[from] conflux_config::EnvError),
    /// A deployment-material variable names something unusable.
    #[error("{0}")]
    Env(#[from] crate::EnvError),
    /// Resolution itself failed.
    #[error("{0}")]
    Resolve(#[from] conflux_config::ConfigError),
    /// The configuration resolved, and validation refused it.
    #[error(
        "configuration invalid: {} error(s) — nothing was started\n{}",
        findings.len(),
        findings.join("\n")
    )]
    InvalidConfiguration {
        /// Every error-severity finding, already rendered.
        findings: Vec<String>,
    },
    /// The `auth` setting and the TLS material disagree.
    #[error("{0}")]
    Tls(#[from] crate::AuthEnforcementError),
    /// `auth = jwt` in production with no key to verify against.
    #[error("{0}")]
    Jwt(#[from] conflux_net::jwt::JwtAuthError),
    /// A backend refused, or could not be reached.
    #[error("{0}")]
    AppState(#[from] crate::AppStateError),
    /// The admin API's binding is not permitted without a token.
    #[error("{0}")]
    AdminBinding(#[from] crate::AdminAuthError),
    /// A variable that should hold a number or an address holds
    /// something else.
    #[error("{var}={value:?} is not valid: {message}")]
    InvalidVar {
        /// The variable.
        var: &'static str,
        /// What it held.
        value: String,
        /// Why it could not be used.
        message: String,
    },
    /// The trusted-reference sidecar is missing, unreachable, or cannot
    /// serve the configured aggregator.
    #[error("{0}")]
    Sidecar(String),
    /// A listener could not take its address — usually another process
    /// already holds that port.
    ///
    /// The message names the knob that moves it. "Address already in
    /// use" is clear about what happened and silent about what to do,
    /// and this is the first failure a new deployment hits: 8080 is a
    /// popular port.
    #[error(
        "cannot bind {addr}: {source}\n  Another process already holds that address. \
         Choose a different one with --http-addr, or set CONFLUX_HTTP_ADDR."
    )]
    Bind {
        /// The address that was refused.
        addr: SocketAddr,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The gRPC server stopped with an error.
    #[error("grpc server failed: {source}")]
    Grpc {
        /// The underlying error.
        source: tonic::transport::Error,
    },
    /// The HTTP admin server stopped with an error.
    #[error("http server failed: {source}")]
    Http {
        /// The underlying error.
        source: std::io::Error,
    },
    /// One of the three long-running tasks panicked or was cancelled.
    #[error("a server task ended unexpectedly: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// Parses a `CONFLUX_*` variable, or `None` when it is unset. A variable
/// that is *set* and unparseable is an error rather than a silent
/// default: naming a value and getting a different one is the failure
/// this configuration layer exists to prevent.
fn parse_env<T>(var: &'static str) -> Result<Option<T>, ServeError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(var) {
        Err(_) => Ok(None),
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|e: T::Err| ServeError::InvalidVar {
                var,
                value,
                message: e.to_string(),
            }),
    }
}

/// Says out loud which profile files were sitting in the profile
/// directory and did not take part in this run.
///
/// The rule and its edge live in `conflux_config::unselected_profiles`;
/// the short version is that only an axis nobody selected is reported,
/// so a deployment that keeps a library of profiles and chooses one is
/// silent. This warns rather than refusing because nothing is violated:
/// the builtins are a legal default and a fresh checkout must still
/// start. The refuse-early rule elsewhere covers a running
/// configuration breaking a promise it made, which is the opposite
/// shape.
fn report_unselected_profiles(
    dir: &std::path::Path,
    topology_name: Option<&str>,
    mode_name: Option<&str>,
    topology_in_force: &str,
    mode_in_force: &str,
) {
    let found = conflux_config::unselected_profiles(dir, topology_name, mode_name);
    if found.is_empty() {
        return;
    }
    for axis in ProfileAxis::ALL {
        let profiles = found.on(axis);
        if profiles.is_empty() {
            continue;
        }
        let in_force = match axis {
            ProfileAxis::Topology => topology_in_force,
            ProfileAxis::Mode => mode_in_force,
        };
        tracing::warn!(
            dir = %dir.display(),
            profiles = %profiles.join(", "),
            "{} is unset, so the builtin {} is in force and these {} profiles were not read. Set {}=<name> to select one.",
            axis.env_var(),
            in_force,
            axis.label(),
            axis.env_var(),
        );
    }
    for unusable in &found.unusable {
        tracing::warn!(
            dir = %dir.display(),
            profile = %unusable.name,
            problem = %unusable.problem,
            "a profile file in the directory cannot be loaded — it would fail the moment anything selected it"
        );
    }
}

/// Resolves the whole deployment from the environment and runs it until
/// `shutdown` completes.
///
/// This is the server binary's entire body, and `cflux server start`
/// calls exactly this — so "the CLI starts the same server" is a fact
/// about the code rather than a promise in a document.
pub async fn run_from_env(
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), ServeError> {
    // Topology and mode select either a builtin or a profile file from
    // CONFLUX_PROFILE_DIR (`<name>.toml`, extending a base via
    // `inherits`). Unset falls back to the builtins; a name that matches
    // *nothing* is a startup error naming what exists, never a silent
    // fallback to `cross_device` — a typo like `cros_silo` would
    // otherwise produce a correctly-logged, wrong deployment.
    let profile_dir =
        std::env::var("CONFLUX_PROFILE_DIR").unwrap_or_else(|_| "profiles".to_string());
    let profile_dir = std::path::Path::new(&profile_dir);

    let topology_name = std::env::var("CONFLUX_TOPOLOGY").ok();
    let topology_profile =
        conflux_config::topology_profile_named(profile_dir, topology_name.as_deref())?;
    let mode_name = std::env::var("CONFLUX_MODE").ok();
    let mode_profile = conflux_config::mode_profile_named(profile_dir, mode_name.as_deref())?;
    // Say the chains out loud once, before the per-parameter lines do.
    if topology_profile.chain.len() > 1 {
        tracing::info!(
            profile = %topology_profile.name,
            chain = %topology_profile.chain.join(" → "),
            dir = %profile_dir.display(),
            "custom topology profile loaded"
        );
    }
    if mode_profile.chain.len() > 1 {
        tracing::info!(
            profile = %mode_profile.name,
            chain = %mode_profile.chain.join(" → "),
            dir = %profile_dir.display(),
            "custom mode profile loaded"
        );
    }
    report_unselected_profiles(
        profile_dir,
        topology_name.as_deref(),
        mode_name.as_deref(),
        &topology_profile.name,
        &mode_profile.name,
    );

    // Downstream startup checks (TLS posture, JWT validation, backend
    // validation) branch on the behavioral mode, which for a custom
    // profile is its `inherits` base — a "production, but…" profile is
    // still production everywhere strictness is decided.
    let mode = mode_profile.base;

    // An optional experiment-level config file. Unset means `None` into
    // the file tier. Set, it is a hard failure if unreadable: an operator who
    // named a config file meant it, and silently continuing with
    // defaults would produce a run whose logged provenance is correct
    // and whose configuration is not what anyone asked for.
    let experiment_file = std::env::var("CONFLUX_EXPERIMENT_CONFIG_PATH").ok();
    let file_overrides = experiment_file
        .as_ref()
        .map(|path| conflux_config::load_experiment_file(std::path::Path::new(path)))
        .transpose()?;
    let file_tier = match (&experiment_file, &file_overrides) {
        (Some(path), Some(overrides)) => Some((path.as_str(), overrides)),
        _ => None,
    };

    // The per-parameter CONFLUX_* variables — the same mapping `cflux
    // config check` reads, so a pre-flight and a real start cannot
    // disagree about what a variable means.
    let env_overrides = conflux_config::overrides_from_env()?;
    let config = conflux_config::resolve_with_profiles(
        &topology_profile,
        &mode_profile,
        file_tier,
        &env_overrides,
        &Overrides::default(),
    )?;

    // Every resolved parameter is logged, with its source, before the
    // server is "ready".
    for line in config.to_log_lines(config.config_log_format.value) {
        println!("{line}");
    }

    // Range and combination validation, after the provenance lines so
    // the two read together: the log says where every value came from,
    // and a finding says which of them cannot work. Warnings are legal
    // but self-contradictory configurations, said out loud; errors are
    // values that guarantee a broken run, and refusing now beats
    // discovering them as behavior in round one.
    let validation = config.validate();
    for finding in &validation.warnings {
        tracing::warn!(parameter = finding.parameter, "[config] {finding}");
    }
    if !validation.errors.is_empty() {
        for finding in &validation.errors {
            eprintln!("[config:error] {finding}");
        }
        return Err(ServeError::InvalidConfiguration {
            findings: validation.errors.iter().map(|f| f.to_string()).collect(),
        });
    }

    // Makes the just-logged `auth` value real — `mode =
    // production` with `auth = mtls` and no TLS material refuses to
    // start here (`resolve_server_tls`'s own fail-fast), rather than
    // silently binding a plaintext gRPC server for a topology whose
    // profile says it should require mTLS.
    let tls_material = crate::tls_material_from_env()?;
    let tls_config = resolve_server_tls(mode, config.auth.value, tls_material)?;
    if tls_config.is_none() && config.auth.value == AuthMode::Mtls {
        tracing::warn!(
            "auth resolved to mtls but no TLS material was configured; falling back to \
             plaintext (research mode only — production would have refused to start)"
        );
    }

    // `CONFLUX_INITIAL_WEIGHTS_DIM`: the real model this deployment trains
    // dictates this, not Conflux (a flat f32 vector is all Conflux ever
    // sees) — e.g. the e2e harnesses set this to their model's actual
    // parameter count. Every
    // client's submitted weights must match this dimension or
    // `AggregatorError::MismatchedLength` rejects the round.
    let initial_weights_dim: usize = parse_env("CONFLUX_INITIAL_WEIGHTS_DIM")?.unwrap_or(4);
    // The `auth = jwt` counterpart to the mTLS check above.
    // Loaded and validated *before* binding, so a production JWT
    // deployment with no key to verify against never starts — the same
    // fail-fast discipline, for the other three topologies' default
    // auth mode.
    let jwt_key = crate::jwt_key_from_env()?;
    validate_jwt_startup(mode, config.auth.value, jwt_key.as_ref())?;
    match (&jwt_key, config.auth.value) {
        (Some(key), AuthMode::Jwt) => {
            tracing::info!(
                algorithm = key.algorithm(),
                "auth = jwt; every register() will be verified against the configured public key"
            );
        }
        (None, AuthMode::Jwt) => {
            tracing::warn!(
                "auth resolved to jwt but no CONFLUX_JWT_PUBLIC_KEY_PATH was configured; \
                 auth_token will not be verified (research mode only — production would \
                 have refused to start)"
            );
        }
        // A key supplied under `auth = mtls` is configuration that does
        // nothing. Said out loud rather than ignored: the operator
        // plainly intended it to be used.
        (Some(_), _) => tracing::warn!(
            "CONFLUX_JWT_PUBLIC_KEY_PATH is set but auth resolved to mtls; \
             no token will be verified"
        ),
        (None, _) => {}
    }

    // "Say so, out loud" applied to a default that is actively
    // dangerous. `clip_radius` has a builtin fallback so the config
    // layer has something to resolve, but there is no value that is
    // right for an unknown model — and the placeholder measured *worse
    // than no defense at all* on a real one. An operator
    // who selected this aggregator and never set the radius has almost
    // certainly not made a choice; say so before serving a round.
    if config.aggregator.value == "centered_clipping"
        && config.clip_radius.source == ConfigSource::BuiltinFallback
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
    let backends = crate::backend_selection_from_env()?;
    let mut state = AppState::connect(config, mode, initial_weights, backends)
        .await?
        .with_jwt_key(jwt_key);

    // Connect to the trusted-reference sidecar, but only if the
    // configured aggregator actually needs one. Asking the aggregator
    // rather than checking whether the env var is set keeps the two
    // failure directions symmetric — a sidecar configured for `fedavg` is
    // ignored, and `fltrust` without a sidecar refuses to start.
    if state.aggregator.requires_trusted_reference() || state.aggregator.requires_candidate_scores()
    {
        state = connect_trusted_reference(state, initial_weights_dim).await?;
    } else if std::env::var("CONFLUX_TRUSTED_REFERENCE_ADDR").is_ok() {
        tracing::warn!(
            aggregator = %state.config.aggregator.value,
            "CONFLUX_TRUSTED_REFERENCE_ADDR is set, but this aggregator does not use a \
             trusted reference — no sidecar connection will be opened"
        );
    }

    let state = Arc::new(state);

    let grpc_addr: SocketAddr = parse_env("CONFLUX_GRPC_ADDR")?
        .unwrap_or_else(|| "127.0.0.1:50051".parse().expect("a literal address"));
    let http_addr: SocketAddr = parse_env("CONFLUX_HTTP_ADDR")?
        .unwrap_or_else(|| "127.0.0.1:8080".parse().expect("a literal address"));

    // The HTTP admin surface's own gate, beside the gRPC ones above.
    // `/admin/allowlist` decides who may participate, so an
    // unauthenticated admin API bound anywhere reachable would undo the
    // authentication on the gRPC port entirely.
    let admin_token = std::env::var("CONFLUX_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .map(AdminToken::new);
    crate::validate_admin_binding(http_addr, admin_token.as_ref())?;
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
    // work, and the round loop finishes the round it is in and then exits.
    // `watch` rather than `broadcast` because the value is a
    // latch — a late subscriber must still see that shutdown was requested,
    // which a missed broadcast message would not give it.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown.await;
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
            .map_err(|source| ServeError::Grpc { source })?;
        tracing::info!("grpc server stopped accepting connections");
        Ok::<(), ServeError>(())
    });

    let http_state = Arc::clone(&state);
    let http_token = admin_token.clone();
    let mut http_shutdown = shutdown_rx.clone();
    // Bound before the spawn, so "that port is already taken" is an
    // error this function returns rather than a panic inside a detached
    // task — the difference between a CLI that explains itself and one
    // that prints a backtrace.
    let listener = tokio::net::TcpListener::bind(http_addr)
        .await
        .map_err(|source| ServeError::Bind {
            addr: http_addr,
            source,
        })?;
    let http = tokio::spawn(async move {
        axum::serve(listener, crate::router(http_state, http_token))
            .with_graceful_shutdown(async move {
                if *http_shutdown.borrow() {
                    return;
                }
                let _ = http_shutdown.changed().await;
            })
            .await
            .map_err(|source| ServeError::Http { source })?;
        tracing::info!("http server stopped accepting connections");
        Ok::<(), ServeError>(())
    });

    let round_state = Arc::clone(&state);
    let health = Arc::clone(&state.round_loop_health);
    let mut round_shutdown = shutdown_rx.clone();
    let rounds = tokio::spawn(async move {
        loop {
            // Checked between rounds, never during one. `run_round` is
            // awaited as a unit below, so a shutdown that arrives mid-round
            // waits for that round to finish rather than abandoning
            // buffered submissions and a half-written checkpoint.
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
                // Retryable errors back off rather than ending the
                // experiment: a `break` here would let one Redis
                // reconnect end the run permanently while the process
                // stayed up. `is_transient` draws the line — see
                // `ServerError` for why it falls where it does.
                Err(e) if e.is_transient() => {
                    let failures = health.record_transient_failure(&e.to_string());
                    let delay = crate::backoff_secs(failures);
                    // `EmptyBatch` is the ordinary "nobody has registered
                    // yet" case and would be alarming at warn level every
                    // two seconds on a freshly-started server.
                    if matches!(
                        e,
                        crate::ServerError::Aggregator(conflux_core::AggregatorError::EmptyBatch)
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

    // A task that failed or panicked surfaces here, rather than being
    // swallowed by a join that discards its results.
    let (grpc, http, rounds) = tokio::join!(grpc, http, rounds);
    grpc.map_err(ServeError::Task)??;
    http.map_err(ServeError::Task)??;
    rounds.map_err(ServeError::Task)?;
    tracing::info!("shutdown complete");
    Ok(())
}

/// scoring-only sidecar finds out before any client has connected.
async fn connect_trusted_reference(
    state: AppState,
    initial_weights_dim: usize,
) -> Result<AppState, ServeError> {
    let addr = crate::trusted_reference_addr().ok_or_else(|| {
        ServeError::Sidecar(format!(
            "aggregator = {:?} requires a trusted-reference sidecar, but \
             CONFLUX_TRUSTED_REFERENCE_ADDR is not set. Start one — \
             `cargo run -p conflux-trusted-reference` — or choose an aggregator that \
             scores from the batch alone.",
            state.config.aggregator.value
        ))
    })?;

    let mut transport = conflux_net::TrustedReferenceTransport::connect(addr.clone())
        .await
        .map_err(|e| {
            ServeError::Sidecar(format!(
                "could not reach the trusted-reference sidecar at {addr}: {e}"
            ))
        })?;

    let capabilities = transport.describe().await.map_err(|e| {
        ServeError::Sidecar(format!(
            "the sidecar at {addr} did not answer Describe: {e}"
        ))
    })?;

    // Gate each capability by what the configured method actually
    // consumes: FLTrust needs reference updates, Zeno needs scoring, and
    // a sidecar that implements only the other one should fail here —
    // at startup, by name — rather than in round one.
    if state.aggregator.requires_trusted_reference() && !capabilities.supports_reference_update {
        return Err(ServeError::Sidecar(format!(
            "the sidecar at {addr} ({}) does not implement reference updates, so it cannot \
             serve aggregator {:?}",
            capabilities.description, state.config.aggregator.value
        )));
    }
    if state.aggregator.requires_candidate_scores() && !capabilities.supports_scoring {
        return Err(ServeError::Sidecar(format!(
            "the sidecar at {addr} ({}) does not implement candidate scoring, so it cannot \
             serve aggregator {:?}",
            capabilities.description, state.config.aggregator.value
        )));
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
            return Err(ServeError::Sidecar(format!(
                "the sidecar at {addr} serves a {dim}-weight model, but this experiment's \
                 model has {experiment_dim} weights"
            )));
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

    Ok(state.with_trusted_reference(transport))
}
