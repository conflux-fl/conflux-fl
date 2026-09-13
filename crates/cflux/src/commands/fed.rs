//! `cflux fed run` — a whole federation on this machine, in one command.
//!
//! # What this is, and what it is not
//!
//! A **local federation**, not a simulation. Every hop is the real one:
//! clients reach their node over the local gRPC hop, nodes reach the
//! server over loopback gRPC, updates are serialized, chunked and
//! reassembled, and the server runs the same buffer, quorum-or-timeout
//! flush, privacy, reputation, aggregation and checkpoint pipeline it
//! runs across machines. What is missing is the network itself — no
//! latency, no loss, no NAT, no bandwidth limit — and, in `task`
//! isolation, process isolation as well.
//!
//! That is why nothing here is labelled a simulation and nothing it
//! produces is marked `simulated`. `cflux sim` is reserved for the tiers
//! that cut the wire, which are the ones that lose client-side DP,
//! chunking and auth along with it.
//!
//! # Scope
//!
//! `CLI_DESIGN.md` ruled multi-client orchestration out of the CLI, on
//! the grounds that spinning up N nodes and N trainers is not something a
//! CLI should hold. That reasoning has changed, not been overruled:
//! `conflux-federation` now holds the orchestration as a library, so this
//! command maps a manifest onto a `FederationConfig` and calls one
//! function. It is the same shape as `cflux server start` over
//! `run_from_env` — a thin verb fronting a library capability, which is
//! exactly the escape hatch the `cflux sim` decision established.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{Args as ClapArgs, Subcommand, ValueEnum};
use conflux_config::{Mode, Overrides, Topology};
use conflux_federation::{FederationConfig, demo};
use serde::Deserialize;
use serde_json::json;

use crate::format::Report;
use crate::{CliError, guide};

#[derive(ClapArgs)]
#[command(after_help = guide("fed"))]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
// `Run` carries eleven options and `Models` carries none. Boxing to even
// them out would buy nothing: this is parsed exactly once per process,
// and clap's derive is happier without the indirection.
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Run a federation on this machine and report what each client did.
    Run(RunArgs),
    /// List the demo clients this binary can run without one being
    /// written.
    Models,
    /// Run **one** demo client against a node's local hop.
    ///
    /// The other half of `--isolation process`: a demo model has to be
    /// reachable as its own process, and it speaks the same
    /// `--address` / `--client-id` / `--rounds` contract as any Python
    /// trainer, so the supervisor treats them identically.
    Client(ClientArgs),
}

#[derive(ClapArgs)]
pub struct ClientArgs {
    /// The node's local hop, e.g. `http://127.0.0.1:47100`.
    #[arg(long)]
    address: String,
    /// This client's identity. Must match the node's, or the server
    /// receives an update from a participant it never registered.
    #[arg(long)]
    client_id: String,
    /// How many rounds to complete before exiting.
    #[arg(long, default_value_t = 1)]
    rounds: usize,
    /// Which demo client. `cflux fed models` lists them.
    #[arg(long, default_value = DEFAULT_MODEL)]
    model: String,
    /// Write this client's score of the global model to this file.
    ///
    /// The same mechanism as `--addr-file`, for the same reason: a
    /// supervisor in another process cannot see what this one learned,
    /// and parsing it out of a log would be guesswork.
    #[arg(long, value_name = "FILE")]
    score_file: Option<PathBuf>,
}

/// How much of a real deployment to reproduce.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    /// Server, nodes and clients as tasks in this process. Fastest, and
    /// loses process isolation on top of the network.
    #[default]
    Task,
    /// Server, nodes and clients each in their own OS process — the same
    /// commands an operator runs on real machines. **Not yet built**;
    /// named here so the flag does not change meaning when it arrives.
    Process,
}

#[derive(ClapArgs)]
pub struct RunArgs {
    /// A `fed.toml` describing the federation. Every field it sets can
    /// be overridden by the flags below.
    manifest: Option<PathBuf>,
    /// How many clients — and therefore how many nodes.
    #[arg(long)]
    clients: Option<usize>,
    /// How many rounds each client completes.
    #[arg(long)]
    rounds: Option<usize>,
    /// Which demo client to run. `cflux fed models` lists them.
    #[arg(long)]
    model: Option<String>,
    /// Aggregation method, validated against the strategy catalog.
    #[arg(long)]
    aggregator: Option<String>,
    /// Client sampling strategy.
    #[arg(long)]
    selector: Option<String>,
    /// `cross_silo`, `cross_device`, `crowdsource` or `edge`.
    #[arg(long)]
    topology: Option<String>,
    /// `research` or `production`.
    #[arg(long)]
    mode: Option<String>,
    /// How much of a real deployment to reproduce.
    #[arg(long, value_enum)]
    isolation: Option<Isolation>,
    /// Where topology and mode profile files live. Defaults to
    /// $CONFLUX_PROFILE_DIR, then `profiles/`.
    #[arg(long)]
    profile_dir: Option<PathBuf>,
    /// Seconds before the run is declared stalled.
    #[arg(long)]
    timeout_secs: Option<u64>,
    /// Where each process writes its log, under `--isolation process`.
    /// Defaults to `cflux-fed/` under the system temp directory.
    #[arg(long, value_name = "DIR")]
    log_dir: Option<PathBuf>,
}

/// A `fed.toml`.
///
/// Deliberately the shape of `baselines/*/baseline.toml` minus its
/// paper-reproduction parts: same `[method]` and `[experiment]` sections,
/// so someone who has read one manifest has read both.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    #[serde(default)]
    federation: FederationSection,
    #[serde(default)]
    method: MethodSection,
    /// Any resolved parameter the config layers accept — `clip_norm`,
    /// `noise_multiplier`, `quorum`, and the rest. Parsed by
    /// `conflux-config` itself rather than re-declared here, so this
    /// file and an experiment config file cannot disagree about what a
    /// key means.
    #[serde(default)]
    experiment: toml::Table,
    #[serde(default)]
    client: ClientSection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FederationSection {
    clients: Option<usize>,
    rounds: Option<usize>,
    isolation: Option<Isolation>,
    topology: Option<String>,
    mode: Option<String>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct MethodSection {
    aggregator: Option<String>,
    selector: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClientSection {
    /// One of the demo clients, by name.
    builtin: Option<String>,
    /// Your own client, as a program and its arguments.
    ///
    /// `--address`, `--client-id` and `--rounds` are appended — the
    /// contract `conflux_client`'s argument parser and
    /// `python/conflux_client/app.py` both already implement, so a
    /// trainer written against either SDK needs no adapter.
    ///
    /// Only `--isolation process` can run one: a task cannot host a
    /// Python interpreter.
    command: Option<Vec<String>>,
}

pub fn run(args: Args) -> Result<Report, CliError> {
    match args.command {
        Command::Run(a) => run_federation(a),
        Command::Models => Ok(models_report()),
        Command::Client(a) => run_one_client(a),
    }
}

fn run_one_client(args: ClientArgs) -> Result<Report, CliError> {
    let model = lookup_model(&args.model)?;
    // Nothing reads this one back, but the demo models write into it —
    // the same slot `fed run` uses to score what was learned.
    let seen = Arc::new(Mutex::new(vec![0.0f32; model.weights_dim]));
    let seen_for_score = Arc::clone(&seen);
    let mut app = (model.make)(client_index(&args.client_id), seen);

    let config = conflux_federation::RunConfig {
        address: args.address.clone(),
        client_id: args.client_id.clone(),
        rounds: args.rounds,
        poll_interval: Duration::from_millis(25),
        ..conflux_federation::RunConfig::default()
    };
    let completed = crate::commands::run::runtime()?
        .block_on(conflux_federation::run_client(&mut app, config))
        .map_err(CliError::Client)?;

    // What this client's last-seen global model is worth, for whoever
    // started it. Written to a file rather than logged, because a
    // supervisor parsing logs is guessing.
    let global = seen_for_score.lock().expect("mutex poisoned").clone();
    let score = model.score.map(|f| f(&global));
    if let (Some(path), Some(score)) = (&args.score_file, score) {
        crate::commands::run::write_report_file(
            path,
            &format!("label={}\nvalue={}\n", score.label, score.value),
        )?;
    }

    Ok(Report::plain(
        format!("{} completed {completed} round(s)\n", args.client_id),
        json!({
            "ok": true,
            "client_id": args.client_id,
            "model": model.name,
            "rounds_completed": completed,
        }),
        0,
    ))
}

/// The trailing number of `client-3`, which is also that client's data
/// shard.
///
/// A demo model's shard is chosen by index, and in process mode the only
/// thing the client is told is its identity — so the index has to come
/// back out of it. `0` for an id with no trailing number, which is a
/// perfectly good shard for a caller who never asked for several.
fn client_index(client_id: &str) -> usize {
    client_id
        .rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

/// The demo model called `name`, or an error listing what exists.
fn lookup_model(name: &str) -> Result<&'static demo::DemoModel, CliError> {
    demo::lookup(name).ok_or_else(|| CliError::Manifest {
        path: "client.builtin".to_string(),
        message: format!(
            "{name:?} is not a demo client — this binary has {}",
            demo::names().join(", ")
        ),
    })
}

fn models_report() -> Report {
    let mut text = String::from("demo clients — each a demo model, not yours\n\n");
    for m in demo::MODELS {
        text.push_str(&format!(
            "  {:<8} {} params   {}\n           {}\n\n",
            m.name, m.weights_dim, m.headline, m.caveat
        ));
    }
    let json = json!({
        "models": demo::MODELS.iter().map(|m| json!({
            "name": m.name,
            "weights_dim": m.weights_dim,
            "headline": m.headline,
            "caveat": m.caveat,
        })).collect::<Vec<_>>()
    });
    Report::plain(text, json, 0)
}

fn load_manifest(path: &Path) -> Result<Manifest, CliError> {
    let raw = std::fs::read_to_string(path).map_err(|source| CliError::Read {
        path: path.display().to_string(),
        source,
    })?;
    toml::from_str(&raw).map_err(|source| CliError::Manifest {
        path: path.display().to_string(),
        message: source.to_string(),
    })
}

fn run_federation(args: RunArgs) -> Result<Report, CliError> {
    let manifest = match &args.manifest {
        Some(path) => load_manifest(path)?,
        None => Manifest::default(),
    };

    // Flags win over the file, the same direction `cflux server start`
    // already layers its own flags over the environment.
    let clients = args
        .clients
        .or(manifest.federation.clients)
        .unwrap_or(DEFAULT_CLIENTS);
    let rounds = args
        .rounds
        .or(manifest.federation.rounds)
        .unwrap_or(DEFAULT_ROUNDS);
    let isolation = args
        .isolation
        .or(manifest.federation.isolation)
        .unwrap_or_default();
    let model_name = args
        .model
        .clone()
        .or_else(|| manifest.client.builtin.clone())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let timeout_secs = args
        .timeout_secs
        .or(manifest.federation.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    let client_command = manifest.client.command.clone();
    if client_command.is_some() && isolation == Isolation::Task {
        return Err(CliError::Manifest {
            path: "client.command".to_string(),
            message: "a command needs its own process — run with --isolation process. \
                      The task tier runs clients as tokio tasks in this process, which \
                      cannot host a program."
                .to_string(),
        });
    }

    // A demo model is still resolved in process mode: it is what
    // `cflux fed client` will run, and its weights_dim is what the
    // server must be initialized to.
    let model = lookup_model(&model_name)?;

    // The experiment table goes through `conflux-config`'s own parser,
    // so `[experiment]` here means exactly what it means in an
    // experiment config file — one schema, one place it is defined.
    let experiment_label = match &args.manifest {
        Some(path) => format!("{} [experiment]", path.display()),
        None => "[experiment]".to_string(),
    };
    let mut from_file: Overrides = if manifest.experiment.is_empty() {
        Overrides::default()
    } else {
        conflux_config::parse_experiment_toml(&manifest.experiment.to_string(), &experiment_label)?
    };
    from_file.aggregator = manifest.method.aggregator.or(from_file.aggregator);
    from_file.selector = manifest.method.selector.or(from_file.selector);

    let mut from_flags = Overrides {
        aggregator: args.aggregator.clone(),
        selector: args.selector.clone(),
        ..Overrides::default()
    };

    // A demo model exists to show convergence, and the framework's
    // privacy defaults — clip every update to an L2 norm of 1, noise at
    // multiplier 1 — are fatal to one: these models have a true norm
    // near 4, so clipping alone discards most of the signal and the run
    // converges to nothing recognizable.
    //
    // Turning them off here, loudly and only when nobody asked for a
    // value, is the honest trade. Leaving them on would produce a demo
    // that "runs" and teaches the reader something false about whether
    // the framework learns. The banner says so, and the provenance lines
    // the server prints name `cli` as the source, so nothing is hidden.
    let privacy_off = from_file.clip_norm.is_none()
        && from_flags.clip_norm.is_none()
        && from_file.noise_multiplier.is_none()
        && from_flags.noise_multiplier.is_none();
    if privacy_off {
        from_flags.clip_norm = Some(1000.0);
        from_flags.noise_multiplier = Some(0.0);
    }

    // Through the profile layer rather than a bare name parse, so a
    // custom profile in `--profile-dir` selects here exactly as it does
    // for `cflux server start` — and a name matching nothing is an error
    // listing what exists, never a silent fall back to `cross_device`.
    let profile_dir = args
        .profile_dir
        .clone()
        .or_else(|| std::env::var("CONFLUX_PROFILE_DIR").ok().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("profiles"));
    let topology_name = args.topology.clone().or(manifest.federation.topology);
    let mode_name = args.mode.clone().or(manifest.federation.mode);
    let topology: Topology =
        conflux_config::topology_profile_named(&profile_dir, topology_name.as_deref())?.base;
    let mode: Mode = conflux_config::mode_profile_named(&profile_dir, mode_name.as_deref())?.base;

    let resolved = {
        let file_tier = args
            .manifest
            .as_ref()
            .map(|p| (p.display().to_string(), from_file));
        conflux_config::resolve(
            topology,
            mode,
            file_tier.as_ref().map(|(label, o)| (label.as_str(), o)),
            &Overrides::default(),
            &from_flags,
        )?
    };

    banner(clients, rounds, isolation, model, &resolved, privacy_off);

    if isolation == Isolation::Process {
        return run_as_processes(
            clients,
            rounds,
            timeout_secs,
            model,
            client_command,
            &resolved,
            mode,
            args.log_dir.clone(),
        );
    }

    let config = FederationConfig {
        nodes: clients,
        rounds,
        timeout: Duration::from_secs(timeout_secs),
        server: Some(conflux_server::ServerConfig {
            resolved,
            mode,
            // The demo client decides this, not the operator: every
            // client's submitted weights must match the server's
            // placeholder initialization or the round is rejected for a
            // length mismatch.
            initial_weights_dim: model.weights_dim,
        }),
        ..FederationConfig::default()
    };

    // One slot every client writes the global model into, because `run`
    // takes ownership of the apps and reading the server's final
    // checkpoint back is not exposed yet.
    let seen = Arc::new(Mutex::new(vec![0.0f32; model.weights_dim]));
    let seen_for_clients = Arc::clone(&seen);
    let make = model.make;
    let summary = crate::commands::run::runtime()?
        .block_on(conflux_federation::run(config, move |i| {
            make(i, Arc::clone(&seen_for_clients))
        }))
        .map_err(CliError::Federation)?;
    let global = seen.lock().expect("mutex poisoned").clone();
    let score = model.score.map(|f| f(&global));

    let mut text = format!(
        "\nfederation complete — {} client(s), {} round(s)\n",
        summary.clients.len(),
        rounds
    );
    for outcome in &summary.clients {
        text.push_str(&format!(
            "  {:<12} {} round(s)\n",
            outcome.client_id, outcome.rounds_completed
        ));
    }
    if let Some(score) = score {
        // "After N-1 aggregations", not "the final model": the last
        // round's own aggregate is written to the server's checkpoint
        // after every client has already been handed its work, so the
        // newest thing any client saw is one round behind.
        text.push_str(&format!(
            "\n  {} after {} aggregation(s): {:.4}\n",
            score.label,
            rounds.saturating_sub(1),
            score.value
        ));
    } else {
        text.push_str(&format!(
            "\n  {} has nothing to score — it does not learn.\n",
            model.name
        ));
    }
    text.push_str(&format!(
        "\n  checkpoints and round records: http://{}/rounds\n",
        summary.server_http_addr
    ));
    let json = json!({
        "ok": true,
        "clients": summary.clients.iter().map(|c| json!({
            "client_id": c.client_id,
            "rounds_completed": c.rounds_completed,
        })).collect::<Vec<_>>(),
        "rounds": rounds,
        "model": model.name,
        "score": score.map(|s| json!({
            "label": s.label,
            "value": s.value,
            "lower_is_better": s.lower_is_better,
            "after_aggregations": rounds.saturating_sub(1),
        })),
        "privacy_defaults_overridden": privacy_off,
        "isolation": "task",
        "simulated": false,
        "grpc_addr": summary.server_grpc_addr.to_string(),
        "http_addr": summary.server_http_addr.to_string(),
    });
    Ok(Report::plain(text, json, 0))
}

const DEFAULT_CLIENTS: usize = 3;
const DEFAULT_ROUNDS: usize = 10;
const DEFAULT_MODEL: &str = "linreg";
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Says what is about to run and what it is not, before it runs.
///
/// To stderr, because `cflux` owns stdout: `--format json` puts a
/// machine-readable report there and a banner interleaved into it would
/// not parse. The same split `init_logging` already makes.
fn banner(
    clients: usize,
    rounds: usize,
    isolation: Isolation,
    model: &demo::DemoModel,
    resolved: &conflux_config::ResolvedConfig,
    privacy_off: bool,
) {
    let lost = match isolation {
        Isolation::Task => "the network, and process isolation",
        Isolation::Process => "the network",
    };
    eprintln!(
        "\nLocal federation — {clients} client(s), {rounds} round(s), isolation = {}\n\
         \n  Real: loopback gRPC, serialization, chunking, quorum-or-timeout, \
         server-side privacy, reputation, aggregation, checkpointing.\n\
         \n  Not real: {lost}. No latency, loss, NAT or bandwidth limit exists here,\n\
         \x20 so nothing this prints is a claim about how a deployment behaves under load.\n\
         \n  Aggregator: {}.  Model: {} — {}\n\
         \n  {}\n",
        match isolation {
            Isolation::Task => "task",
            Isolation::Process => "process",
        },
        resolved.aggregator.value,
        model.name,
        model.headline,
        model.caveat,
    );
    if privacy_off {
        eprintln!(
            "  Differential privacy is OFF: clip_norm = 1000, noise_multiplier = 0.\n\
             \x20 The framework's defaults (clip 1, noise 1) do not converge on a \
             {}-parameter demo,\n\
             \x20 so this command turns them off unless you set either one. The \
             accountant will warn\n\
             \x20 every round that there is no guarantee; that is it doing its job.\n",
            model.weights_dim
        );
    }
}

/// `--isolation process`: every participant in its own OS process.
///
/// The commands are this binary's own — `cflux server start`,
/// `cflux node start`, `cflux fed client` — which is what makes the
/// printed recipe worth anything: those are the same commands an
/// operator runs on real machines, and an installed `cflux` needs no
/// sibling binaries on disk to run them.
#[allow(clippy::too_many_arguments)]
fn run_as_processes(
    clients: usize,
    rounds: usize,
    timeout_secs: u64,
    model: &'static demo::DemoModel,
    client_command: Option<Vec<String>>,
    resolved: &conflux_config::ResolvedConfig,
    mode: Mode,
    log_dir: Option<PathBuf>,
) -> Result<Report, CliError> {
    use conflux_federation::process::{ProcessParticipant, ProcessPlan, Spawn, own_exe};

    let exe_err = |source| CliError::Read {
        path: "this executable".to_string(),
        source,
    };

    // The children read their configuration from the environment, the
    // same way every Conflux binary does — so the resolved values are
    // handed over as `CONFLUX_*` rather than re-derived per child, and a
    // child's own provenance lines name where each came from.
    let mut server = own_exe(&["server", "start"]).map_err(exe_err)?;
    for (var, value) in server_env(resolved, mode, model) {
        server = server.env(var, value);
    }

    let log_dir = log_dir.unwrap_or_else(|| std::env::temp_dir().join("cflux-fed"));
    let participants = (0..clients)
        .map(|i| {
            let client_id = format!("client-{i}");
            let client = match &client_command {
                Some(parts) if !parts.is_empty() => {
                    let mut c = Spawn::new(&parts[0]);
                    for a in &parts[1..] {
                        c = c.arg(a);
                    }
                    c
                }
                // No command given: run one of this binary's own demo
                // clients, which speaks exactly the same contract.
                _ => own_exe(&["fed", "client"])
                    .map_err(exe_err)?
                    .flag("--model", model.name)
                    .flag("--score-file", log_dir.join(format!("{client_id}.score"))),
            };
            Ok(ProcessParticipant {
                client_id,
                node: own_exe(&["node", "start"]).map_err(exe_err)?,
                client,
            })
        })
        .collect::<Result<Vec<_>, CliError>>()?;

    let plan = ProcessPlan {
        server,
        participants,
        rounds,
        startup_timeout: Duration::from_secs(60),
        run_timeout: Duration::from_secs(timeout_secs),
        log_dir: log_dir.clone(),
    };

    let summary = conflux_federation::process::run(plan).map_err(CliError::ProcessFederation)?;

    let mut text = format!(
        "\nfederation complete — {} client(s), {} round(s)\n",
        summary.clients.len(),
        rounds
    );
    for outcome in &summary.clients {
        text.push_str(&format!("  {:<12} exited cleanly\n", outcome.client_id));
    }
    // Each client scored the global model it last saw and wrote the
    // number to a file, because a supervisor in another process has no
    // other way to see what was learned.
    let scores = collect_scores(&log_dir, &summary);
    match scores.as_slice() {
        [] => text.push_str("\n  No score: this model has none, or no client reported one.\n"),
        values => {
            let mean = values.iter().map(|(_, v)| v).sum::<f32>() / values.len() as f32;
            text.push_str(&format!(
                "\n  {} across {} client(s): {:.4}\n",
                values[0].0,
                values.len(),
                mean
            ));
            // Each client scored the global model *it* last saw, and two
            // clients can be a round apart. Averaging them is honest;
            // calling it "the" score would not be.
            text.push_str("  (each scored the global model it last saw, so this is a mean)\n");
        }
    }
    text.push_str(&format!(
        "\n  round records: http://{}/rounds\n",
        summary.server_http_addr
    ));
    text.push_str(&format!("\n  logs: {}\n", log_dir.display()));
    text.push_str("\n  What ran, which is also what you would run on real machines:\n");
    for line in &summary.recipe {
        text.push_str(&format!("    {line}\n"));
    }

    let json = json!({
        "ok": true,
        "clients": summary.clients.iter().map(|c| json!({
            "client_id": c.client_id,
            "exit_code": c.exit_code,
        })).collect::<Vec<_>>(),
        "rounds": rounds,
        "model": model.name,
        "isolation": "process",
        "simulated": false,
        "grpc_addr": summary.server_grpc_addr.to_string(),
        "http_addr": summary.server_http_addr.to_string(),
        "log_dir": log_dir.display().to_string(),
        "recipe": summary.recipe,
        "score": scores.first().map(|(label, _)| json!({
            "label": label,
            "value": scores.iter().map(|(_, v)| v).sum::<f32>() / scores.len() as f32,
            "clients_reporting": scores.len(),
        })),
    });
    Ok(Report::plain(text, json, 0))
}

/// Reads back what each client scored, skipping any that reported none.
fn collect_scores(
    log_dir: &Path,
    summary: &conflux_federation::process::ProcessSummary,
) -> Vec<(String, f32)> {
    summary
        .clients
        .iter()
        .filter_map(|c| {
            let path = log_dir.join(format!("{}.score", c.client_id));
            let text = std::fs::read_to_string(path).ok()?;
            let mut label = None;
            let mut value = None;
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("label=") {
                    label = Some(rest.to_string());
                } else if let Some(rest) = line.strip_prefix("value=") {
                    value = rest.parse().ok();
                }
            }
            Some((label?, value?))
        })
        .collect()
}

/// The `CONFLUX_*` variables a child server needs to resolve to exactly
/// what this command already resolved.
fn server_env(
    resolved: &conflux_config::ResolvedConfig,
    mode: Mode,
    model: &demo::DemoModel,
) -> Vec<(String, String)> {
    vec![
        ("CONFLUX_MODE".into(), mode.label().to_string()),
        (
            "CONFLUX_AGGREGATOR".into(),
            resolved.aggregator.value.clone(),
        ),
        ("CONFLUX_SELECTOR".into(), resolved.selector.value.clone()),
        (
            "CONFLUX_CLIP_NORM".into(),
            resolved.clip_norm.value.to_string(),
        ),
        (
            "CONFLUX_NOISE_MULTIPLIER".into(),
            resolved.noise_multiplier.value.to_string(),
        ),
        (
            "CONFLUX_INITIAL_WEIGHTS_DIM".into(),
            model.weights_dim.to_string(),
        ),
    ]
}
