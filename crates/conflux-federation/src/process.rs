//! The same federation, with every participant in its own OS process.
//!
//! [`crate::Federation`] runs a server, N nodes and N clients as tokio
//! tasks. This runs them as processes — the tier above it in
//! fidelity, and the only one that can host a client the orchestrator
//! cannot link: a Python trainer, or anything else that speaks the
//! `--address` / `--client-id` contract.
//!
//! What it buys over the task tier is **process isolation**: a client
//! that segfaults takes down a client, not the run. What it costs is
//! startup time and memory — a Python trainer is hundreds of megabytes
//! resident, where a tokio task is hundreds of bytes.
//!
//! # Ports are still nobody's guess
//!
//! The task tier binds every listener itself, because it can. Separate
//! processes cannot share a listener without passing file descriptors,
//! which is Unix-only and awkward. The temptation is then to have the
//! supervisor pick ports — and every way of doing that is a guess:
//! base-plus-index collides, and checking a port is free before the
//! child binds it is a race rather than a check.
//!
//! So the child binds, and *reports*. Each participant is started on
//! `127.0.0.1:0` with an `--addr-file`; it binds, writes the address the
//! OS actually gave it, and only then serves. The supervisor polls for
//! that file. There is no window in which anybody has guessed anything,
//! and it works identically on every platform.
//!
//! # Why a supervisor rather than a shell script
//!
//! Four failure modes that a script gets wrong and this does not: a
//! child that dies during startup is reported as *that child's* error
//! rather than as a timeout somewhere else; every child's output goes to
//! a named log so a failure can quote it; nothing is left running when
//! the supervisor exits, however it exits; and "the port answers" is
//! never mistaken for "our process answered", because the address came
//! from the child itself.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How to start one process.
#[derive(Debug, Clone)]
pub struct Spawn {
    /// The program. Usually the supervisor's own executable — a
    /// federation on one machine runs the same commands an operator runs
    /// on several.
    pub program: OsString,
    /// Its arguments, before anything this module appends.
    pub args: Vec<OsString>,
    /// Environment to set for it, on top of the supervisor's own.
    pub env: Vec<(OsString, OsString)>,
}

impl Spawn {
    /// A program with no arguments and no extra environment.
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
        }
    }

    /// Appends one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends a flag and its value.
    pub fn flag(self, flag: &str, value: impl Into<OsString>) -> Self {
        self.arg(flag).arg(value)
    }

    /// Sets one environment variable.
    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// What an operator would type to run this by hand.
    ///
    /// Printed rather than merely executed, because a local federation
    /// that spawns the same commands a deployment uses can hand you the
    /// deployment recipe for free.
    pub fn recipe(&self) -> String {
        let mut out = String::new();
        for (k, v) in &self.env {
            out.push_str(&format!("{}={} ", k.to_string_lossy(), v.to_string_lossy()));
        }
        out.push_str(&self.program.to_string_lossy());
        for a in &self.args {
            out.push(' ');
            out.push_str(&a.to_string_lossy());
        }
        out
    }
}

/// One participant: a node, and the client that attaches to its local hop.
#[derive(Debug, Clone)]
pub struct ProcessParticipant {
    /// The identity both halves present. They must match — the local hop
    /// absorbs a client's registration but passes `submit_delta` through
    /// unchanged, so a mismatch sends the server an update from a
    /// participant it never registered.
    pub client_id: String,
    /// The node. `--addr-file` and the server's address are appended.
    pub node: Spawn,
    /// The client. `--address`, `--client-id` and `--rounds` are
    /// appended, which is the contract `conflux_client`'s own argument
    /// parser and `python/conflux_client/app.py` both implement.
    pub client: Spawn,
}

/// Everything needed to run one federation as processes.
#[derive(Debug, Clone)]
pub struct ProcessPlan {
    /// The server. `--addr-file` is appended.
    pub server: Spawn,
    /// Every participant, in order.
    pub participants: Vec<ProcessParticipant>,
    /// Rounds each client is asked for.
    pub rounds: usize,
    /// How long any one participant may take to report its address and
    /// start serving.
    pub startup_timeout: Duration,
    /// How long the clients have, all together, before the run is
    /// declared stalled.
    pub run_timeout: Duration,
    /// Where each process's output goes, one file per participant.
    pub log_dir: PathBuf,
}

/// What one client process did.
#[derive(Debug, Clone)]
pub struct ProcessOutcome {
    /// Which participant.
    pub index: usize,
    /// Its identity.
    pub client_id: String,
    /// Its exit code, or `None` if a signal ended it.
    pub exit_code: Option<i32>,
}

/// What a completed run did.
#[derive(Debug, Clone)]
pub struct ProcessSummary {
    /// One entry per client, in index order.
    pub clients: Vec<ProcessOutcome>,
    /// The server's gRPC address, as the server itself reported it.
    pub server_grpc_addr: SocketAddr,
    /// The server's HTTP admin address.
    pub server_http_addr: SocketAddr,
    /// Every command that was run, in order, as an operator would type
    /// it. This is the deployment recipe.
    pub recipe: Vec<String>,
}

/// Why a process federation could not start, or did not finish.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// A child could not be started at all — usually the program is not
    /// on `PATH`.
    #[error("could not start {label} ({program}): {source}")]
    Spawn {
        /// Which participant.
        label: String,
        /// What was being run.
        program: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A child exited before it reported an address.
    #[error("{label} exited during startup with {status}\n{log}")]
    DiedDuringStartup {
        /// Which participant.
        label: String,
        /// How it ended.
        status: String,
        /// The tail of its log, so the reason is in the error.
        log: String,
    },
    /// A child neither reported an address nor exited.
    #[error("{label} did not report an address within {timeout:?}\n{log}")]
    StartupTimeout {
        /// Which participant.
        label: String,
        /// How long it was given.
        timeout: Duration,
        /// The tail of its log.
        log: String,
    },
    /// A child reported something that is not an address.
    #[error("{label} wrote {contents:?} to its address file, which has no {key}= line")]
    UnreadableAddrFile {
        /// Which participant.
        label: String,
        /// The line that was expected.
        key: &'static str,
        /// What was there instead.
        contents: String,
    },
    /// A client exited with a failure.
    #[error("client {index} ({client_id}) failed with {status}\n{log}")]
    ClientFailed {
        /// Which participant.
        index: usize,
        /// Its identity.
        client_id: String,
        /// How it ended.
        status: String,
        /// The tail of its log.
        log: String,
    },
    /// The clients did not all finish in time.
    #[error(
        "the federation did not finish within {timeout:?}: {} of {total} client(s) were still \
         running — {}",
        unfinished.len(),
        unfinished.join(", ")
    )]
    Stalled {
        /// How long the run was given.
        timeout: Duration,
        /// How many clients there were.
        total: usize,
        /// The identities still going.
        unfinished: Vec<String>,
    },
    /// A log file or the log directory could not be created.
    #[error("could not open {path}: {source}")]
    Log {
        /// The path.
        path: String,
        /// The underlying error.
        source: std::io::Error,
    },
}

/// Every child this federation started, killed when it goes out of scope.
///
/// `Drop` rather than an explicit teardown: a supervisor that leaves a
/// server holding a port behind when it returns early — on an error, on
/// a panic, on `?` from anywhere — is worse than one that never started.
/// `Drop` runs on all three.
struct Children {
    running: Vec<(String, Child)>,
}

impl Children {
    fn new() -> Self {
        Self {
            running: Vec::new(),
        }
    }

    fn push(&mut self, label: &str, child: Child) -> &mut Child {
        self.running.push((label.to_string(), child));
        &mut self.running.last_mut().expect("just pushed").1
    }
}

impl Drop for Children {
    fn drop(&mut self) {
        for (label, child) in &mut self.running {
            if matches!(child.try_wait(), Ok(None)) {
                tracing::debug!(label, "stopping");
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

/// The last `lines` lines of a log, for putting a failure's reason inside
/// the failure.
fn tail(path: &Path, lines: usize) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return format!("  (no log at {})", path.display());
    };
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return format!("  (log at {} is not text)", path.display());
    }
    let kept: Vec<&str> = text.lines().rev().take(lines).collect();
    if kept.is_empty() {
        return format!("  ({} is empty)", path.display());
    }
    let mut out = format!("  --- {} ---\n", path.display());
    for line in kept.into_iter().rev() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn spawn(label: &str, s: &Spawn, log_dir: &Path) -> Result<(Child, PathBuf), ProcessError> {
    let log_path = log_dir.join(format!("{label}.log"));
    let log = std::fs::File::create(&log_path).map_err(|source| ProcessError::Log {
        path: log_path.display().to_string(),
        source,
    })?;
    let errors = log.try_clone().map_err(|source| ProcessError::Log {
        path: log_path.display().to_string(),
        source,
    })?;

    let mut cmd = Command::new(&s.program);
    cmd.args(&s.args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(errors));
    for (k, v) in &s.env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().map_err(|source| ProcessError::Spawn {
        label: label.to_string(),
        program: s.program.to_string_lossy().into_owned(),
        source,
    })?;
    Ok((child, log_path))
}

/// Waits for `addr_file` to appear and reads `key=<addr>` out of it.
///
/// The child's liveness is checked alongside, because a file that never
/// appears asks the wrong question: a child that died has a reason, and
/// reporting a timeout instead would hide it.
fn await_addr(
    label: &str,
    child: &mut Child,
    addr_file: &Path,
    key: &'static str,
    log_path: &Path,
    timeout: Duration,
) -> Result<SocketAddr, ProcessError> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(contents) = std::fs::read_to_string(addr_file) {
            if let Some(line) = contents.lines().find(|l| l.starts_with(&format!("{key}="))) {
                let raw = &line[key.len() + 1..];
                if let Ok(addr) = raw.parse() {
                    return Ok(addr);
                }
            }
            // Present but not yet what we need. Only an error once the
            // child has stopped writing, which the checks below decide.
            if !contents.is_empty() && Instant::now() >= deadline {
                return Err(ProcessError::UnreadableAddrFile {
                    label: label.to_string(),
                    key,
                    contents,
                });
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(ProcessError::DiedDuringStartup {
                    label: label.to_string(),
                    status: status.to_string(),
                    log: tail(log_path, 25),
                });
            }
            Ok(None) => {}
            Err(_) => {}
        }
        if Instant::now() >= deadline {
            return Err(ProcessError::StartupTimeout {
                label: label.to_string(),
                timeout,
                log: tail(log_path, 25),
            });
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A bare HTTP/1.1 `GET /health`, true on a `200`.
///
/// Hand-written rather than pulled from an HTTP client: one request, on
/// loopback, of which four bytes are read. A dependency for that would
/// cost more than it explains.
fn health_ok(addr: SocketAddr) -> bool {
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(500))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let request = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.starts_with("HTTP/1.1 200")
}

/// Runs a whole federation as processes and returns when every client
/// has exited.
///
/// Blocking, deliberately: this is process supervision, not async I/O.
/// Every child is killed when this returns, whichever way it returns.
pub fn run(plan: ProcessPlan) -> Result<ProcessSummary, ProcessError> {
    std::fs::create_dir_all(&plan.log_dir).map_err(|source| ProcessError::Log {
        path: plan.log_dir.display().to_string(),
        source,
    })?;

    let mut children = Children::new();
    let mut recipe = Vec::new();

    // --- the server ------------------------------------------------------
    let server_addr_file = plan.log_dir.join("server.addr");
    let _ = std::fs::remove_file(&server_addr_file);
    let server = plan
        .server
        .clone()
        .flag("--addr-file", &server_addr_file)
        .flag("--grpc-addr", "127.0.0.1:0")
        .flag("--http-addr", "127.0.0.1:0");
    recipe.push(server.recipe());
    let (child, server_log) = spawn("server", &server, &plan.log_dir)?;
    let server_child = children.push("server", child);

    let grpc_addr = await_addr(
        "server",
        server_child,
        &server_addr_file,
        "grpc",
        &server_log,
        plan.startup_timeout,
    )?;
    let http_addr = await_addr(
        "server",
        server_child,
        &server_addr_file,
        "http",
        &server_log,
        plan.startup_timeout,
    )?;

    // The address is known, and the port is bound — but a server that
    // refuses its configuration should be reported here, by its own
    // error, rather than as N identical transport failures a minute
    // later.
    let deadline = Instant::now() + plan.startup_timeout;
    while !health_ok(http_addr) {
        if let Ok(Some(status)) = server_child.try_wait() {
            return Err(ProcessError::DiedDuringStartup {
                label: "server".to_string(),
                status: status.to_string(),
                log: tail(&server_log, 25),
            });
        }
        if Instant::now() >= deadline {
            return Err(ProcessError::StartupTimeout {
                label: "server".to_string(),
                timeout: plan.startup_timeout,
                log: tail(&server_log, 25),
            });
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let grpc_url = format!("http://{grpc_addr}");
    tracing::info!(%grpc_addr, %http_addr, "server up");

    // --- the nodes -------------------------------------------------------
    let mut node_urls = Vec::with_capacity(plan.participants.len());
    for (i, p) in plan.participants.iter().enumerate() {
        let label = format!("node-{i}");
        let addr_file = plan.log_dir.join(format!("{label}.addr"));
        let _ = std::fs::remove_file(&addr_file);
        let node = p
            .node
            .clone()
            .flag("--addr-file", &addr_file)
            .flag("--server", &grpc_url)
            .flag("--client-id", &p.client_id)
            .flag("--local-addr", "127.0.0.1:0");
        recipe.push(node.recipe());
        let (child, log) = spawn(&label, &node, &plan.log_dir)?;
        let node_child = children.push(&label, child);
        let addr = await_addr(
            &label,
            node_child,
            &addr_file,
            "local",
            &log,
            plan.startup_timeout,
        )?;
        node_urls.push(format!("http://{addr}"));
    }

    // --- the clients -----------------------------------------------------
    //
    // Started last and tracked separately: they are the ones that finish,
    // and the run is over when they have.
    let mut clients = Vec::with_capacity(plan.participants.len());
    for (i, p) in plan.participants.iter().enumerate() {
        let label = format!("client-{i}");
        let client = p
            .client
            .clone()
            .flag("--address", &node_urls[i])
            .flag("--client-id", &p.client_id)
            .flag("--rounds", plan.rounds.to_string());
        recipe.push(client.recipe());
        let (child, log) = spawn(&label, &client, &plan.log_dir)?;
        clients.push((i, p.client_id.clone(), child, log));
    }

    // --- wait ------------------------------------------------------------
    let deadline = Instant::now() + plan.run_timeout;
    let mut outcomes: Vec<Option<ProcessOutcome>> = vec![None; clients.len()];
    loop {
        let mut still_running = Vec::new();
        for (index, client_id, child, log) in clients.iter_mut() {
            if outcomes[*index].is_some() {
                continue;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err(ProcessError::ClientFailed {
                            index: *index,
                            client_id: client_id.clone(),
                            status: status.to_string(),
                            log: tail(log, 25),
                        });
                    }
                    outcomes[*index] = Some(ProcessOutcome {
                        index: *index,
                        client_id: client_id.clone(),
                        exit_code: status.code(),
                    });
                }
                Ok(None) => still_running.push(client_id.clone()),
                Err(_) => still_running.push(client_id.clone()),
            }
        }
        if still_running.is_empty() {
            break;
        }
        if Instant::now() >= deadline {
            // Loud, and naming who. A run that returned partial results
            // here is how a number from an unfinished federation ends up
            // recorded as if it were a finished one.
            return Err(ProcessError::Stalled {
                timeout: plan.run_timeout,
                total: clients.len(),
                unfinished: still_running,
            });
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    // Clients are done, so the server and nodes have nothing left to do.
    // `children` kills them on the way out of this function.
    Ok(ProcessSummary {
        clients: outcomes.into_iter().flatten().collect(),
        server_grpc_addr: grpc_addr,
        server_http_addr: http_addr,
        recipe,
    })
}

/// `Spawn` for this process's own executable.
///
/// A local federation runs the same commands an operator runs on real
/// machines, which is what makes its printed recipe worth anything —
/// and it means an installed `cflux` needs no sibling binaries on disk.
pub fn own_exe(subcommand: &[&str]) -> Result<Spawn, std::io::Error> {
    let exe = std::env::current_exe()?;
    let mut s = Spawn::new(exe);
    for part in subcommand {
        s = s.arg(OsStr::new(part));
    }
    Ok(s)
}
