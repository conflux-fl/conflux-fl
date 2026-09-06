//! Running a real federation for a baseline's Python edge.
//!
//! This replaces a per-harness `run_demo.sh`. The shell versions worked,
//! and four copies of them drifted: each had its own port handling, its
//! own health gate, its own cleanup trap. Here there is one, and the
//! cleanup is a `Drop` impl rather than a trap that a `set -e` can skip.
//!
//! What it starts, in order: data preparation, a `conflux-server`, one
//! `conflux-node` per client, one trainer per node, and an evaluator.
//! What it reads back is the evaluator's `held_out_accuracy` line — the
//! server's own model as a client sees it, not a local reconstruction.

use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What a recipe needs to build its data and model — the `[experiment]`
/// table of a manifest, which already carried exactly this.
pub(crate) struct Recipe<'a> {
    pub(crate) model: &'a str,
    pub(crate) dataset: &'a str,
    pub(crate) partition: &'a str,
}

/// How to run one federation.
pub(crate) struct Plan<'a> {
    pub(crate) recipe: Recipe<'a>,
    pub(crate) aggregator: &'a str,
    pub(crate) clients: u32,
    pub(crate) attackers: u32,
    pub(crate) rounds: u32,
    pub(crate) no_reputation: bool,
}

/// Every process this federation started, killed when it goes out of
/// scope.
///
/// A `Drop` rather than a cleanup call at the end: the run can fail at
/// any of a dozen points, and a server left holding port 50051 turns one
/// failed baseline into every later baseline failing too.
#[derive(Default)]
struct Processes {
    children: Vec<Child>,
}

impl Processes {
    fn push(&mut self, child: Child) -> &mut Child {
        self.children.push(child);
        self.children.last_mut().expect("just pushed")
    }
}

impl Drop for Processes {
    fn drop(&mut self) {
        for child in &mut self.children {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Local ports this federation uses. Every one is overridable, because a
/// developer machine is allowed to already be using 8080.
struct Ports {
    grpc: u16,
    admin: u16,
    node_base: u16,
}

impl Ports {
    fn from_env() -> Self {
        fn var(name: &str, default: u16) -> u16 {
            std::env::var(name)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        Self {
            grpc: var("CONFLUX_GRPC_PORT", 50051),
            admin: var("CONFLUX_ADMIN_PORT", 8080),
            node_base: var("CONFLUX_NODE_PORT_BASE", 47100),
        }
    }
}

/// Refuses to start when something already holds a port this federation
/// needs.
///
/// Checked before spawning, because a port that answers is not evidence
/// that *our* process answered it. An unrelated service on 8080 will
/// satisfy any startup probe instantly — before our server has even
/// tried to bind — and the run then fails much later as a transport
/// error from the first node, pointing at the wrong thing entirely.
fn ensure_free(port: u16, what: &str, var: &str) -> Result<(), String> {
    if TcpStream::connect(("127.0.0.1", port)).is_ok() {
        return Err(format!(
            "127.0.0.1:{port} is already in use and the {what} needs it — stop whatever holds \
             it, or set {var}"
        ));
    }
    Ok(())
}

/// Waits until `child` is accepting connections on `port`.
///
/// A TCP probe rather than an HTTP health check: the runner needs to
/// know the listener is up, and adding an HTTP client to a build tool to
/// learn the same thing costs a dependency for nothing.
///
/// The child's liveness is checked alongside the port because a port
/// alone answers the wrong question. Something *else* on the machine can
/// hold 8080 — which is how this first failed — and then the probe
/// succeeds against a stranger while our server is already dead, turning
/// a one-line bind error into a transport error from the first node.
/// Watching the process instead reports the bind failure where it
/// happened.
fn wait_for_child_port(child: &mut Child, port: u16, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => return Err(format!("it exited early ({status})")),
            Ok(None) => {}
            Err(e) => return Err(format!("cannot check whether it is running: {e}")),
        }
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err("it did not accept a connection in time".to_string())
}

/// The last few lines of a child's log, for a failure message.
///
/// A process that failed to start said why on its stderr, and a runner
/// that discards it turns "the server never came up" into a mystery.
/// Every child writes to its own file, and every failure path quotes the
/// tail of the one that is likely to explain it.
fn tail(work_dir: &Path, name: &str) -> String {
    let Ok(text) = std::fs::read_to_string(work_dir.join(name)) else {
        return String::new();
    };
    let lines: Vec<&str> = text.lines().rev().take(8).collect();
    if lines.is_empty() {
        return String::new();
    }
    let mut out = format!("\n  --- {name} ---\n");
    for line in lines.iter().rev() {
        out.push_str(&format!("  {line}\n"));
    }
    out
}

/// The Python interpreter to run the harness with — the client's venv
/// when it exists, since that is where PyTorch is installed.
fn python(repo_root: &Path) -> PathBuf {
    let venv = repo_root.join("python/conflux_client/.venv/bin/python");
    if venv.exists() {
        venv
    } else {
        PathBuf::from("python3")
    }
}

/// Prepares the data and returns the model's parameter count, which the
/// server needs as `CONFLUX_INITIAL_WEIGHTS_DIM`.
fn prepare(repo_root: &Path, plan: &Plan, work_dir: &Path) -> Result<usize, String> {
    let output = Command::new(python(repo_root))
        .args([
            "-m",
            "_harness.prepare",
            "--model",
            plan.recipe.model,
            "--dataset",
            plan.recipe.dataset,
            "--partition",
            plan.recipe.partition,
        ])
        .args(["--clients", &plan.clients.to_string()])
        .args(["--out-dir", &work_dir.display().to_string()])
        .current_dir(repo_root.join("baselines"))
        .output()
        .map_err(|e| format!("could not run the data preparation: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "data preparation failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // The last JSON line carries the model dimension; the rest is the
    // human-readable account of what was written.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .find_map(|line| {
            let line = line.trim();
            let rest = line.strip_prefix("{\"model_dim\": ")?;
            rest.split(',').next()?.trim().parse::<usize>().ok()
        })
        .ok_or_else(|| "data preparation printed no model dimension".to_string())
}

/// Runs one federation and returns the final `held_out_accuracy`.
pub(crate) fn run(repo_root: &Path, plan: &Plan) -> Result<f64, String> {
    let ports = Ports::from_env();
    // Up front, before the data preparation spends a minute on a run
    // that cannot succeed.
    ensure_free(ports.grpc, "server's gRPC transport", "CONFLUX_GRPC_PORT")?;
    ensure_free(ports.admin, "server's admin API", "CONFLUX_ADMIN_PORT")?;
    for i in 0..plan.clients {
        ensure_free(
            ports.node_base + i as u16,
            &format!("local hop for client-{i}"),
            "CONFLUX_NODE_PORT_BASE",
        )?;
    }

    let work_dir = repo_root.join("target").join("baseline-work");
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("cannot create a work dir: {e}"))?;

    let dim = prepare(repo_root, plan, &work_dir)?;
    println!("  prepared {} clients; model dimension {dim}", plan.clients);

    let mut procs = Processes::default();
    let server_bin = repo_root.join("target/debug/conflux-server");
    let node_bin = repo_root.join("target/debug/conflux-node");
    for bin in [&server_bin, &node_bin] {
        if !bin.exists() {
            return Err(format!(
                "{} is missing — run `cargo build -p conflux-server -p conflux-node` first",
                bin.display()
            ));
        }
    }

    // The reputation filter is a separate defense from the aggregator's
    // own. A robustness baseline turns it off so the number measures the
    // method the paper describes rather than two filters in series.
    let min_reputation = if plan.no_reputation { "0.0" } else { "0.3" };
    let server = procs.push(
        Command::new(&server_bin)
            .env("CONFLUX_TOPOLOGY", "cross_device")
            .env("CONFLUX_MODE", "research")
            .env("CONFLUX_AGGREGATOR", plan.aggregator)
            .env("CONFLUX_ROBUST_BYZANTINE_FRACTION", "0.3")
            .env("CONFLUX_MIN_REPUTATION_SCORE", min_reputation)
            .env("CONFLUX_QUORUM", plan.clients.to_string())
            .env("CONFLUX_SCAFFOLD_NUM_CLIENTS", plan.clients.to_string())
            .env("CONFLUX_ROUND_TIMEOUT_SECS", "60")
            // The harnesses measure convergence, not privacy: a clip
            // wide enough to be inert and no noise, so a number that
            // moves moved because of the aggregator.
            .env("CONFLUX_CLIP_NORM", "1000")
            .env("CONFLUX_NOISE_MULTIPLIER", "0")
            .env("CONFLUX_INITIAL_WEIGHTS_DIM", dim.to_string())
            .env("CONFLUX_GRPC_ADDR", format!("127.0.0.1:{}", ports.grpc))
            .env("CONFLUX_HTTP_ADDR", format!("127.0.0.1:{}", ports.admin))
            .env("RUST_LOG", "warn")
            .stdout(Stdio::null())
            .stderr(Stdio::from(
                std::fs::File::create(work_dir.join("server.log"))
                    .map_err(|e| format!("cannot open the server log: {e}"))?,
            ))
            .spawn()
            .map_err(|e| format!("could not start conflux-server: {e}"))?,
    );
    if let Err(why) = wait_for_child_port(server, ports.admin, Duration::from_secs(30)) {
        return Err(format!(
            "conflux-server never listened on 127.0.0.1:{}: {why} — something else may hold that \
             port (set CONFLUX_ADMIN_PORT, and CONFLUX_GRPC_PORT){}",
            ports.admin,
            tail(&work_dir, "server.log")
        ));
    }

    for i in 0..plan.clients {
        let port = ports.node_base + i as u16;
        let node = procs.push(
            Command::new(&node_bin)
                .env("CONFLUX_CLIENT_ID", format!("client-{i}"))
                .env("CONFLUX_LOCAL_ADDR", format!("127.0.0.1:{port}"))
                .env(
                    "CONFLUX_SERVER_ADDR",
                    format!("http://127.0.0.1:{}", ports.grpc),
                )
                .env("RUST_LOG", "warn")
                .stdout(Stdio::null())
                .stderr(Stdio::from(
                    std::fs::File::create(work_dir.join(format!("node_{i}.log")))
                        .map_err(|e| format!("cannot open a node log: {e}"))?,
                ))
                .spawn()
                .map_err(|e| format!("could not start conflux-node {i}: {e}"))?,
        );
        if let Err(why) = wait_for_child_port(node, port, Duration::from_secs(20)) {
            return Err(format!(
                "conflux-node {i} never listened on 127.0.0.1:{port}: {why}{}",
                tail(&work_dir, &format!("node_{i}.log"))
            ));
        }
    }

    let py = python(repo_root);
    for i in 0..plan.clients {
        let port = ports.node_base + i as u16;
        let mut cmd = Command::new(&py);
        cmd.args(["-m", "_harness.trainer", "--model", plan.recipe.model])
            .args([
                "--shard",
                &work_dir.join(format!("shard_{i}.pt")).display().to_string(),
            ])
            .args(["--address", &format!("127.0.0.1:{port}")])
            .args(["--client-id", &format!("client-{i}")])
            .args(["--rounds", &plan.rounds.to_string()])
            .current_dir(repo_root.join("baselines"))
            .stdout(Stdio::null())
            // Kept, not discarded: a trainer that cannot start is the
            // most likely reason a run produces no metric, and throwing
            // its explanation away turns a five-second diagnosis into a
            // guess. Written to a file per trainer rather than inherited
            // so five clients do not interleave into nonsense.
            .stderr(Stdio::from(
                std::fs::File::create(work_dir.join(format!("trainer_{i}.log")))
                    .map_err(|e| format!("cannot open a trainer log: {e}"))?,
            ));
        // The attackers are the last `attackers` clients, so a run with
        // none is byte-identical to a clean run.
        if i >= plan.clients.saturating_sub(plan.attackers) && plan.attackers > 0 {
            cmd.arg("--poison");
        }
        procs.push(
            cmd.spawn()
                .map_err(|e| format!("could not start trainer {i}: {e}"))?,
        );
    }

    // The evaluator registers like any other client and never submits,
    // so what it scores is the server's model as clients receive it.
    let evaluator = procs.push(
        Command::new(&py)
            .args(["-m", "_harness.evaluator", "--model", plan.recipe.model])
            .args(["--address", &format!("127.0.0.1:{}", ports.node_base)])
            .args([
                "--held-out",
                &work_dir.join("held_out.pt").display().to_string(),
            ])
            .args(["--rounds", &plan.rounds.to_string()])
            .args(["--timeout", "900"])
            .current_dir(repo_root.join("baselines"))
            .stdout(Stdio::piped())
            .stderr(Stdio::from(
                std::fs::File::create(work_dir.join("evaluator.log"))
                    .map_err(|e| format!("cannot open the evaluator log: {e}"))?,
            ))
            .spawn()
            .map_err(|e| format!("could not start the evaluator: {e}"))?,
    );

    let stdout = evaluator
        .stdout
        .take()
        .ok_or("the evaluator has no stdout")?;
    let mut last = None;
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        println!("  {line}");
        if let Some(pos) = line.find("held_out_accuracy=") {
            let rest = &line[pos + "held_out_accuracy=".len()..];
            let value: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = value.parse::<f64>() {
                last = Some(v);
            }
        }
    }
    last.ok_or_else(|| {
        let mut message = String::from("the evaluator never reported held_out_accuracy");
        for name in ["evaluator.log", "trainer_0.log", "server.log"] {
            message.push_str(&tail(&work_dir, name));
        }
        message
    })
}
