//! `cflux server start` and `cflux node start`.
//!
//! Deliberately thin, and thin in a specific way: each hands off to the
//! same `run_from_env` its binary calls, so "the CLI starts the real
//! server" is a fact about the code rather than a claim. Flags are a
//! convenience over the environment those functions already read — they
//! set the variable and then get out of the way, which is why a flag and
//! its `CONFLUX_*` counterpart can never mean different things.

use clap::{Args as ClapArgs, Subcommand};
use serde_json::json;

use crate::format::Report;
use crate::{CliError, guide};

#[derive(ClapArgs)]
#[command(after_help = guide("server-start"))]
pub struct ServerArgs {
    #[command(subcommand)]
    command: ServerCommand,
}

#[derive(Subcommand)]
enum ServerCommand {
    /// Run the round pipeline, the gRPC transport, and the admin API
    /// until interrupted.
    Start(ServerStart),
}

#[derive(ClapArgs)]
struct ServerStart {
    /// Topology: a builtin or a profile file name. Sets $CONFLUX_TOPOLOGY.
    #[arg(long)]
    topology: Option<String>,
    /// Mode: a builtin or a profile file name. Sets $CONFLUX_MODE.
    #[arg(long)]
    mode: Option<String>,
    /// Experiment overrides file. Sets $CONFLUX_EXPERIMENT_CONFIG_PATH.
    #[arg(long = "config", value_name = "FILE")]
    config_file: Option<String>,
    /// Profile directory. Sets $CONFLUX_PROFILE_DIR.
    #[arg(long, value_name = "DIR")]
    profile_dir: Option<String>,
    /// gRPC listen address. Sets $CONFLUX_GRPC_ADDR.
    #[arg(long)]
    grpc_addr: Option<String>,
    /// HTTP admin listen address. Sets $CONFLUX_HTTP_ADDR.
    #[arg(long)]
    http_addr: Option<String>,
}

#[derive(ClapArgs)]
#[command(after_help = guide("node-start"))]
pub struct NodeArgs {
    #[command(subcommand)]
    command: NodeCommand,
}

#[derive(Subcommand)]
enum NodeCommand {
    /// Register with a server and serve the local hop a `ClientApp`
    /// connects to, until interrupted.
    Start(NodeStart),
}

#[derive(ClapArgs)]
struct NodeStart {
    /// Upstream server. Sets $CONFLUX_SERVER_ADDR.
    #[arg(long)]
    server: Option<String>,
    /// This node's id at registration. Sets $CONFLUX_CLIENT_ID.
    #[arg(long)]
    client_id: Option<String>,
    /// Local address the `ClientApp` connects to. Sets $CONFLUX_LOCAL_ADDR.
    #[arg(long)]
    local_addr: Option<String>,
    /// `pull` or `push`. Sets $CONFLUX_CONNECTION_MODE.
    #[arg(long)]
    connection_mode: Option<String>,
}

/// Puts a flag's value into the environment the run functions read.
///
/// Called before any runtime exists, which is what makes it sound: the
/// process is still single-threaded here, so nothing can be reading the
/// environment while it changes.
fn export(var: &str, value: Option<&String>) {
    if let Some(value) = value {
        // SAFETY: single-threaded — `main` has not built a runtime or
        // spawned a thread at this point, and every reader of these
        // variables runs after the runtime starts below.
        unsafe { std::env::set_var(var, value) };
    }
}

/// A runtime for a process whose whole job is to run one server.
fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|source| CliError::Runtime { source })
}

/// Resolves when the process is asked to stop — the same posture both
/// binaries have: close the listeners rather than vanish.
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
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    // Printed rather than traced: `cflux` initializes no subscriber, so a
    // `tracing` event here would go nowhere. The servers it starts do
    // their own logging.
    tokio::select! {
        _ = ctrl_c => eprintln!("received Ctrl-C; shutting down"),
        _ = terminate => eprintln!("received SIGTERM; shutting down"),
    }
}

pub fn run_server(args: ServerArgs) -> Result<Report, CliError> {
    let ServerCommand::Start(a) = args.command;
    export("CONFLUX_TOPOLOGY", a.topology.as_ref());
    export("CONFLUX_MODE", a.mode.as_ref());
    export("CONFLUX_EXPERIMENT_CONFIG_PATH", a.config_file.as_ref());
    export("CONFLUX_PROFILE_DIR", a.profile_dir.as_ref());
    export("CONFLUX_GRPC_ADDR", a.grpc_addr.as_ref());
    export("CONFLUX_HTTP_ADDR", a.http_addr.as_ref());

    runtime()?
        .block_on(conflux_server::run_from_env(shutdown_signal()))
        .map_err(CliError::Server)?;
    Ok(Report::plain(
        "server stopped\n".to_string(),
        json!({ "ok": true, "stopped": "server" }),
        0,
    ))
}

pub fn run_node(args: NodeArgs) -> Result<Report, CliError> {
    let NodeCommand::Start(a) = args.command;
    export("CONFLUX_SERVER_ADDR", a.server.as_ref());
    export("CONFLUX_CLIENT_ID", a.client_id.as_ref());
    export("CONFLUX_LOCAL_ADDR", a.local_addr.as_ref());
    export("CONFLUX_CONNECTION_MODE", a.connection_mode.as_ref());

    runtime()?
        .block_on(conflux_node::run_from_env(shutdown_signal()))
        .map_err(CliError::Node)?;
    Ok(Report::plain(
        "node stopped\n".to_string(),
        json!({ "ok": true, "stopped": "node" }),
        0,
    ))
}
