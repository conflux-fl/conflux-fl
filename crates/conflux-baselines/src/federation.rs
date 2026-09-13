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

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Local SGD steps each client takes per round.
///
/// Stated here rather than left to the trainer's own default, because
/// the centralized baseline has to spend the same total budget for the
/// comparison to mean anything — and a default in Python that this file
/// silently depended on would drift without either side noticing.
pub(crate) const LOCAL_STEPS_PER_ROUND: u32 = 30;

/// What a robust aggregator assumes about how many clients are
/// Byzantine, unless a manifest says otherwise.
///
/// A deliberate over-estimate: a defense sized for more adversaries than
/// are present is conservative, and most of the catalog degrades
/// gracefully when it is wrong in that direction. It does not suit every
/// method — see `Plan::byzantine_fraction`.
pub(crate) const DEFAULT_BYZANTINE_FRACTION: f64 = 0.3;

/// The seed a single run uses, matching `_harness/prepare.py`'s own
/// default.
///
/// Stated rather than left implicit because every baseline's expected
/// value was measured at this seed — passing a different one silently
/// moves the number a reproduction is checked against.
pub(crate) const DEFAULT_SEED: u32 = 42;

/// What a recipe needs to build its data and model — the `[experiment]`
/// table of a manifest, which already carried exactly this.
pub(crate) struct Recipe<'a> {
    pub(crate) model: &'a str,
    pub(crate) dataset: &'a str,
    pub(crate) partition: &'a str,
    /// Only meaningful for the `dirichlet` partition, where it sets how
    /// skewed the label distribution is. `None` leaves the harness's own
    /// default alone rather than restating it here, so the two cannot
    /// drift apart.
    pub(crate) dirichlet_alpha: Option<f64>,
}

/// How to run one federation.
pub(crate) struct Plan<'a> {
    pub(crate) recipe: Recipe<'a>,
    pub(crate) aggregator: &'a str,
    pub(crate) clients: u32,
    pub(crate) attackers: u32,
    pub(crate) rounds: u32,
    pub(crate) no_reputation: bool,
    /// Which repetition this is.
    ///
    /// It reaches two places, and both matter. `prepare` uses it to draw
    /// the subsample and the partition, so each seed sees different data;
    /// each trainer gets one derived from it, so each seed also walks a
    /// different SGD trajectory over that data. Vary only the first and a
    /// "multi-seed" sweep replays one trajectory per shard, which
    /// understates the real run-to-run spread.
    pub(crate) seed: u32,
    /// What the aggregator should assume, overriding
    /// [`DEFAULT_BYZANTINE_FRACTION`].
    ///
    /// Bulyan is why this exists. Its guarantee holds only for
    /// `n >= 4f + 3`, and with `f` fixed at 30% of `n` that inequality
    /// has no solution at any client count — so at the default it would
    /// run, silently, outside the regime its paper describes. The
    /// implementation floors and clamps rather than refusing, which
    /// makes that failure quiet rather than loud.
    pub(crate) byzantine_fraction: Option<f64>,
}

/// One round as the evaluator saw it.
#[derive(Clone, Copy)]
pub(crate) struct RoundMetric {
    pub(crate) round: u32,
    pub(crate) accuracy: f64,
    pub(crate) loss: f64,
    /// The worst client's accuracy on its own data, and the spread across
    /// clients. `None` when the evaluator was not given the shards.
    ///
    /// A pooled mean cannot see who it is failing, and the fairness
    /// methods make their claim about this distribution rather than about
    /// its average — without these two numbers that claim is not
    /// measurable.
    pub(crate) client_acc_min: Option<f64>,
    pub(crate) client_acc_std: Option<f64>,
}

/// What one federation produced.
pub(crate) struct Outcome {
    pub(crate) rounds: Vec<RoundMetric>,
}

impl Outcome {
    /// The number a baseline is judged on: the last round the evaluator
    /// managed to report.
    pub(crate) fn final_accuracy(&self) -> f64 {
        self.rounds.last().map(|r| r.accuracy).unwrap_or(f64::NAN)
    }
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
        .args(["--seed", &plan.seed.to_string()])
        .args(match plan.recipe.dirichlet_alpha {
            Some(alpha) => vec!["--dirichlet-alpha".to_string(), alpha.to_string()],
            None => vec![],
        })
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

/// Runs one federation and returns every round the evaluator reported.
///
/// The orchestration itself lives in `conflux_federation::process`, the
/// same supervisor `cflux fed run --isolation process` uses. This
/// function's job is the parts that are specific to reproducing a paper:
/// preparing the shards, describing the participants, and reading the
/// evaluator's numbers back.
pub(crate) fn run(repo_root: &Path, plan: &Plan) -> Result<Outcome, String> {
    use conflux_federation::process::{Observer, ProcessParticipant, ProcessPlan, Spawn};

    let work_dir = work_dir(repo_root);
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir).map_err(|e| format!("cannot create a work dir: {e}"))?;

    let dim = prepare(repo_root, plan, &work_dir)?;
    println!("  prepared {} clients; model dimension {dim}", plan.clients);

    // One binary instead of two, and the same one an operator installs.
    // `cflux server start` and `cflux node start` hand off to exactly the
    // `run_from_env` the dedicated binaries call, so nothing about the
    // run changes — but `--addr-file` only exists here, and that is what
    // lets every listener bind port 0.
    let cflux = repo_root.join("target/debug/cflux");
    if !cflux.exists() {
        return Err(format!(
            "{} is missing — run `cargo build -p cflux` first",
            cflux.display()
        ));
    }

    // The reputation filter is a separate defense from the aggregator's
    // own. A robustness baseline turns it off so the number measures the
    // method the paper describes rather than two filters in series.
    let min_reputation = if plan.no_reputation { "0.0" } else { "0.3" };
    let server = Spawn::new(&cflux)
        .arg("server")
        .arg("start")
        .env("CONFLUX_TOPOLOGY", "cross_device")
        .env("CONFLUX_MODE", "research")
        .env("CONFLUX_AGGREGATOR", plan.aggregator)
        .env(
            "CONFLUX_ROBUST_BYZANTINE_FRACTION",
            plan.byzantine_fraction
                .unwrap_or(DEFAULT_BYZANTINE_FRACTION)
                .to_string(),
        )
        .env("CONFLUX_MIN_REPUTATION_SCORE", min_reputation)
        .env("CONFLUX_QUORUM", plan.clients.to_string())
        .env("CONFLUX_SCAFFOLD_NUM_CLIENTS", plan.clients.to_string())
        .env("CONFLUX_ROUND_TIMEOUT_SECS", "60")
        // The harnesses measure convergence, not privacy: a clip wide
        // enough to be inert and no noise, so a number that moves moved
        // because of the aggregator.
        .env("CONFLUX_CLIP_NORM", "1000")
        .env("CONFLUX_NOISE_MULTIPLIER", "0")
        .env("CONFLUX_INITIAL_WEIGHTS_DIM", dim.to_string())
        .env("RUST_LOG", "warn");

    let py = python(repo_root);
    let baselines_dir = repo_root.join("baselines");
    let participants: Vec<ProcessParticipant> = (0..plan.clients)
        .map(|i| {
            let mut client = Spawn::new(&py)
                .arg("-m")
                .arg("_harness.trainer")
                .flag("--model", plan.recipe.model)
                .flag("--shard", work_dir.join(format!("shard_{i}.pt")))
                .flag("--steps", LOCAL_STEPS_PER_ROUND.to_string())
                // Derived rather than shared: every client reseeding to
                // the same value would make five trainers draw the same
                // batches, which is a different experiment from the one
                // intended. The model init stays shared — federated
                // learning requires it — and only the sampling varies.
                .flag("--trainer-seed", (plan.seed * 1000 + i).to_string())
                .env("PYTHONPATH", &baselines_dir);
            // The attackers are the last `attackers` clients, so a run
            // with none is byte-identical to a clean run.
            if i >= plan.clients.saturating_sub(plan.attackers) && plan.attackers > 0 {
                client = client.arg("--poison");
            }
            ProcessParticipant {
                client_id: format!("client-{i}"),
                node: Spawn::new(&cflux)
                    .arg("node")
                    .arg("start")
                    .env("RUST_LOG", "warn"),
                client,
            }
        })
        .collect();

    // The evaluator registers like any other client and never submits,
    // so what it scores is the server's model as clients receive it. It
    // shares client 0's local hop rather than taking a node of its own,
    // which would put a sixth participant in the registry and change the
    // quorum the round is waiting for.
    let evaluator = Observer {
        client_id: "evaluator".to_string(),
        node: 0,
        stream_stdout: true,
        spawn: Spawn::new(&py)
            .arg("-m")
            .arg("_harness.evaluator")
            .flag("--model", plan.recipe.model)
            .flag("--held-out", work_dir.join("held_out.pt"))
            .flag("--rounds", plan.rounds.to_string())
            .flag("--timeout", "900")
            // Every client's own training data, so each round reports
            // the spread across clients beside the pooled number.
            .arg("--shards")
            .env("PYTHONPATH", &baselines_dir),
    };
    let evaluator = (0..plan.clients).fold(evaluator, |mut e, i| {
        e.spawn = e.spawn.arg(work_dir.join(format!("shard_{i}.pt")));
        e
    });

    let mut rounds: Vec<RoundMetric> = Vec::new();
    let outcome = conflux_federation::process::run_watching(
        ProcessPlan {
            server,
            participants,
            observers: vec![evaluator],
            rounds: plan.rounds as usize,
            startup_timeout: Duration::from_secs(60),
            run_timeout: Duration::from_secs(1800),
            log_dir: work_dir.clone(),
        },
        |line| {
            println!("  {line}");
            // A round line carries all three; anything else is commentary.
            let (Some(round), Some(accuracy), Some(loss)) = (
                field(line, "round="),
                field(line, "held_out_accuracy="),
                field(line, "held_out_loss="),
            ) else {
                return;
            };
            let metric = RoundMetric {
                round: round as u32,
                accuracy,
                loss,
                client_acc_min: field(line, "client_acc_min="),
                client_acc_std: field(line, "client_acc_std="),
            };
            // The evaluator polls, so it can report the same round twice;
            // the later reading is the one taken.
            match rounds.iter_mut().find(|r| r.round == metric.round) {
                Some(existing) => *existing = metric,
                None => rounds.push(metric),
            }
        },
    );

    if let Err(e) = outcome {
        return Err(format!("{e}"));
    }
    if rounds.is_empty() {
        let mut message = String::from("the evaluator never reported held_out_accuracy");
        for name in ["observer-0-evaluator.log", "client-0.log", "server.log"] {
            message.push_str(&tail(&work_dir, name));
        }
        return Err(message);
    }
    rounds.sort_by_key(|r| r.round);
    Ok(Outcome { rounds })
}

/// Where a run materializes its data and logs. One directory, so a
/// sweep's centralized baseline reads the same `pooled.pt` the
/// federation trained against rather than a second copy.
pub(crate) fn work_dir(repo_root: &Path) -> PathBuf {
    repo_root.join("target").join("baseline-work")
}

/// One `name=value` number out of an evaluator line.
fn field(line: &str, name: &str) -> Option<f64> {
    let rest = &line[line.find(name)? + name.len()..];
    let value: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    value.parse().ok()
}

/// Trains the recipe's model on the pooled data — the non-federated bar
/// a federated run is compared against.
///
/// Independent of aggregator, partition and attack, so a sweep computes
/// it once and reuses it across the whole grid rather than paying for an
/// identical number on every combination, as the shell version did.
pub(crate) fn centralized_baseline(
    repo_root: &Path,
    recipe: &Recipe,
    work_dir: &Path,
    total_steps: u32,
) -> Result<f64, String> {
    let output = Command::new(python(repo_root))
        .args(["-m", "_harness.centralized", "--model", recipe.model])
        .args([
            "--pooled",
            &work_dir.join("pooled.pt").display().to_string(),
        ])
        .args([
            "--held-out",
            &work_dir.join("held_out.pt").display().to_string(),
        ])
        .args(["--total-steps", &total_steps.to_string()])
        .current_dir(repo_root.join("baselines"))
        .output()
        .map_err(|e| format!("could not run the centralized baseline: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "the centralized baseline failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| field(line, "held_out_accuracy="))
        .ok_or_else(|| "the centralized baseline reported no accuracy".to_string())
}
