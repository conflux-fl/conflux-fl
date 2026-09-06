//! Server binary — integrates the library crates into the round pipeline.
//!
//! Configuration arrives three ways: topology/mode from
//! `CONFLUX_TOPOLOGY`/`CONFLUX_MODE` (a builtin name or a profile file
//! under `CONFLUX_PROFILE_DIR`), an optional experiment file via
//! `CONFLUX_EXPERIMENT_CONFIG_PATH`, and per-parameter `CONFLUX_*` env
//! vars (`overrides_from_env` below). There is no CLI-flag tier yet.
//!
//! Backend selection is env-var driven too, deliberately kept separate
//! from `conflux-config`'s `Overrides`: a Redis URL is a deployment
//! detail, not an experiment-tuning parameter.
//!
//! Node auth needs no separate wiring here: `require_node_auth` is a
//! regular `conflux-config` parameter (already covered by the
//! provenance-log loop below), and `AppState::connect` derives the
//! allow-list backend from `CONFLUX_REGISTRY_BACKEND` itself — one fewer
//! env var rather than a fully independent backend axis.
//!
//! `CONFLUX_GRPC_ADDR`/`CONFLUX_HTTP_ADDR` (below) exist because both
//! listeners default to `127.0.0.1`, which is unreachable from a separate
//! container (e.g. a FastAPI/Django backend calling the HTTP admin API
//! from its own container) unless it shares this process's network
//! namespace — see `https://confluxfl.dev/guides/web-app-integration/`.
//! Defaults stay loopback-only; binding the admin API wider requires
//! `CONFLUX_ADMIN_TOKEN`, enforced at startup.
//!
//! `CONFLUX_REPUTATION_FILTER_ENABLED`: reputation filtering is opt-in,
//! defaulting to `false`. A `CosineScorer` applied unconditionally in
//! front of every aggregator would be an uncited filter no paper (Krum,
//! Trimmed Mean, Median, ...) asks for, and would mask the aggregator's
//! own behavior. `CONFLUX_MIN_REPUTATION_SCORE` controls the threshold
//! used *when* it is turned on.

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // Everything this binary does lives in the library, so that `cflux
    // server start` runs the same server rather than a second
    // implementation of it. What stays here is what only a binary can
    // decide: how to log, and what to do when startup fails.
    if let Err(e) = conflux_server::run_from_env(shutdown_signal()).await {
        eprintln!("conflux-server: {e}");
        std::process::exit(1);
    }
}

/// Resolves when the process is asked to stop: Ctrl-C on any platform, or
/// `SIGTERM` on Unix.
///
/// `SIGTERM` is the one that matters in production — it is what
/// `docker stop`, a Kubernetes eviction, and systemd all send first, with
/// `SIGKILL` following after a grace period. Without a handler the
/// default disposition terminates the process immediately, so the grace
/// period would be spent doing nothing and the round in flight lost.
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
