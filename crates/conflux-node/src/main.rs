//! Client binary — Rust-side networking/orchestration, hands training off
//! to a local `ClientApp` (Python or Rust) over loopback gRPC.
//!
//! No CLI/config resolution — address/id come from env vars, matching
//! `conflux-server`'s `main.rs`.
//!
//! `CONFLUX_MODE`/`CONFLUX_ALLOW_STUB_CLIENT`/
//! `CONFLUX_CLIENT_APP_KIND` gate startup via `startup_guard` — see that
//! module.
//!
//! `CONFLUX_CONNECTION_MODE` (`push`/`pull`) picks which upstream
//! transport to open. It defaults to `pull`, which is *not* the same as
//! defaulting to any one topology's posture: three of the four topologies
//! resolve to pull, and a node started without being told anything about
//! its deployment should take the conservative option (ask when ready)
//! rather than hold a connection open on the assumption it's a trusted
//! silo. A `cross_silo` deployment sets this to `push` explicitly, the
//! same way it already has to be told the server address.

#[tokio::main]
async fn main() {
    // `RUST_LOG` when set, otherwise INFO rather than the library
    // default of ERROR — for the reason `conflux-server`'s `main`
    // spells out: what the node says about its own decisions, including
    // the stub-client guard, is worth hearing without being asked for.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // The node's whole body lives in the library, so `cflux node start`
    // runs this node rather than a second implementation of it.
    if let Err(e) = conflux_node::run_from_env(shutdown_signal()).await {
        eprintln!("conflux-node: {e}");
        std::process::exit(1);
    }
}

/// Resolves when the process is asked to stop: Ctrl-C on any platform, or
/// `SIGTERM` on Unix.
///
/// The node's shutdown is simpler than the server's — it holds no round
/// state and writes no checkpoints, so there is nothing to drain. What it
/// gains is an *orderly* stop: the local listener closes rather than the
/// process vanishing, so a `ClientApp` mid-call sees a closed
/// connection instead of a reset, and `docker stop` produces exit code 0
/// rather than a signal death.
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
