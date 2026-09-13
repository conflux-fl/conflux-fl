//! `cflux server start` and `cflux node start`.
//!
//! Deliberately thin, and thin in a specific way: each hands off to the
//! same `run_from_env` its binary calls, so "the CLI starts the real
//! server" is a fact about the code rather than a claim. Flags are a
//! convenience over the environment those functions already read — they
//! set the variable and then get out of the way, which is why a flag and
//! its `CONFLUX_*` counterpart can never mean different things.

use std::path::{Path, PathBuf};

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
    /// Write the addresses actually bound to this file, then serve.
    ///
    /// Two lines, `grpc=<addr>` and `http=<addr>`. The point of it is
    /// `--grpc-addr 127.0.0.1:0`: the OS picks free ports, and this is
    /// how whoever started this process finds out which ones. Without
    /// it a supervisor has to pick the ports itself, and every way of
    /// doing that without binding is a guess.
    #[arg(long, value_name = "FILE")]
    addr_file: Option<PathBuf>,
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
    /// Write the local-hop address actually bound to this file, then
    /// serve. See `server start --addr-file`.
    #[arg(long, value_name = "FILE")]
    addr_file: Option<PathBuf>,
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
pub(crate) fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
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

    // Printed rather than traced, and it stays that way: this line is
    // the CLI talking about itself, not the server talking about a
    // decision, so it belongs beside the command's own output.
    tokio::select! {
        _ = ctrl_c => eprintln!("received Ctrl-C; shutting down"),
        _ = terminate => eprintln!("received SIGTERM; shutting down"),
    }
}

/// Sends the server's and node's own `tracing` events somewhere.
///
/// `server start` and `node start` do not spawn a binary — they call the
/// same `run_from_env` the binaries call, in this process. So without a
/// subscriber here, everything the framework says out loud went nowhere:
/// quorum-or-timeout, every rejected update and its score, cumulative
/// epsilon, and the startup warnings about an unauthenticated admin API
/// or JWT auth with no key. The command that the tutorial ends on was
/// the quietest way to run the server.
///
/// To stderr, unlike the binaries' stdout, because `cflux` owns stdout:
/// `--format json` puts a machine-readable report there, and log lines
/// interleaved into it would not parse.
///
/// `try_init` rather than `init`: failing to install a subscriber is not
/// a reason to refuse to start a server.
fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .try_init();
}

/// Binds both of the server's listeners from the same environment
/// `serve` would read, so `--addr-file` reports exactly what will be
/// served.
async fn bind_server_listeners() -> Result<conflux_server::ServerListeners, CliError> {
    async fn bind(var: &str, fallback: &str) -> Result<tokio::net::TcpListener, CliError> {
        let raw = std::env::var(var).unwrap_or_else(|_| fallback.to_string());
        let addr: std::net::SocketAddr = raw.parse().map_err(|_| CliError::Manifest {
            path: var.to_string(),
            message: format!("{raw:?} is not an address"),
        })?;
        tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|source| CliError::Write {
                path: addr.to_string(),
                source,
            })
    }
    Ok(conflux_server::ServerListeners {
        grpc: bind("CONFLUX_GRPC_ADDR", "127.0.0.1:50051").await?,
        http: bind("CONFLUX_HTTP_ADDR", "127.0.0.1:8080").await?,
    })
}

/// Writes `contents` to `path` via a temporary file and a rename.
///
/// The reader is a supervisor polling for this file to appear, and a
/// partial read of a half-written file would hand it an address that is
/// a *prefix* of the real one — which parses, and then connects to the
/// wrong port. A rename is atomic on every platform this ships for, so
/// the file is either absent or complete.
pub(crate) fn write_report_file(path: &Path, contents: &str) -> Result<(), CliError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).map_err(|source| CliError::Write {
        path: tmp.display().to_string(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| CliError::Write {
        path: path.display().to_string(),
        source,
    })
}

pub fn run_server(args: ServerArgs) -> Result<Report, CliError> {
    let ServerCommand::Start(a) = args.command;
    init_logging();
    export("CONFLUX_TOPOLOGY", a.topology.as_ref());
    export("CONFLUX_MODE", a.mode.as_ref());
    export("CONFLUX_EXPERIMENT_CONFIG_PATH", a.config_file.as_ref());
    export("CONFLUX_PROFILE_DIR", a.profile_dir.as_ref());
    export("CONFLUX_GRPC_ADDR", a.grpc_addr.as_ref());
    export("CONFLUX_HTTP_ADDR", a.http_addr.as_ref());

    let rt = runtime()?;
    match a.addr_file {
        // Bind here rather than inside `serve`, so the real addresses can
        // be reported before anything runs. This is what `run_from_env_on`
        // taking listeners is for: whoever binds is the only one who can
        // read back what the OS chose.
        Some(path) => {
            let listeners = rt.block_on(bind_server_listeners())?;
            let addr_of = |l: &tokio::net::TcpListener| {
                l.local_addr().map_err(|source| CliError::Write {
                    path: path.display().to_string(),
                    source,
                })
            };
            let line = format!(
                "grpc={}\nhttp={}\n",
                addr_of(&listeners.grpc)?,
                addr_of(&listeners.http)?
            );
            write_report_file(&path, &line)?;
            rt.block_on(conflux_server::run_from_env_on(
                listeners,
                shutdown_signal(),
            ))
            .map_err(CliError::Server)?;
        }
        None => rt
            .block_on(conflux_server::run_from_env(shutdown_signal()))
            .map_err(CliError::Server)?,
    }
    Ok(Report::plain(
        "server stopped\n".to_string(),
        json!({ "ok": true, "stopped": "server" }),
        0,
    ))
}

pub fn run_node(args: NodeArgs) -> Result<Report, CliError> {
    let NodeCommand::Start(a) = args.command;
    init_logging();
    export("CONFLUX_SERVER_ADDR", a.server.as_ref());
    export("CONFLUX_CLIENT_ID", a.client_id.as_ref());
    export("CONFLUX_LOCAL_ADDR", a.local_addr.as_ref());
    export("CONFLUX_CONNECTION_MODE", a.connection_mode.as_ref());

    let rt = runtime()?;
    match a.addr_file {
        Some(path) => {
            let config = conflux_node::NodeConfig::from_env().map_err(CliError::Node)?;
            let listener = rt
                .block_on(tokio::net::TcpListener::bind(config.local_addr))
                .map_err(|source| CliError::Write {
                    path: config.local_addr.to_string(),
                    source,
                })?;
            let bound = listener.local_addr().map_err(|source| CliError::Write {
                path: path.display().to_string(),
                source,
            })?;
            write_report_file(&path, &format!("local={bound}\n"))?;
            rt.block_on(conflux_node::run_on(listener, config, shutdown_signal()))
                .map_err(CliError::Node)?;
        }
        None => rt
            .block_on(conflux_node::run_from_env(shutdown_signal()))
            .map_err(CliError::Node)?,
    }
    Ok(Report::plain(
        "node stopped\n".to_string(),
        json!({ "ok": true, "stopped": "node" }),
        0,
    ))
}
