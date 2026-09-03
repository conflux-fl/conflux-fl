//! `conflux-baselines` — run and verify Conflux FL's published-paper
//! reproductions. Design + manifest schema:
//! `conflux-fl-internal/docs/BASELINES.md`.
//!
//! A baseline is a *recipe*, not an implementation: `baseline.toml` names a
//! cataloged method plus the paper's setup and expected result, and one or
//! two **client edges** — Python (PyTorch) and/or Rust (Burn). This runner
//! validates the method against the strategy registry, drives the chosen
//! edge's harness, and asserts the achieved metric against the paper's
//! number.

use std::path::{Path, PathBuf};
use std::process::{Command, exit};

use conflux_config::{StrategyKind, entries};
use serde::Deserialize;

// Stable exit codes so CI and scripts can branch on them.
const EXIT_FAIL: i32 = 1; // reproduction miss or config/usage error
const EXIT_UNKNOWN_METHOD: i32 = 2; // a named method is not in the catalog

const METRIC: &str = "held_out_accuracy"; // both edges report this token

// ---------------------------------------------------------------------------
// Manifest schema — mirrors baselines/<name>/baseline.toml. `#[serde(default)]`
// makes a table/field optional.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Manifest {
    paper: Paper,
    method: Method,
    experiment: Experiment,
    #[serde(default)]
    scenario: Scenario,
    clients: Clients,
}

#[derive(Deserialize)]
struct Paper {
    title: String,
    authors: String,
    venue: String,
    url: String,
}

#[derive(Deserialize)]
struct Method {
    aggregator: String,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    privacy_mechanism: Option<String>,
}

#[derive(Deserialize)]
struct Experiment {
    dataset: String,
    model: String,
    partition: String,
}

/// A Byzantine-robustness reproduction runs with attackers present; a plain
/// convergence reproduction leaves this defaulted (no attackers).
#[derive(Deserialize, Default)]
struct Scenario {
    #[serde(default)]
    attackers: u32,
    #[serde(default)]
    no_reputation: bool,
}

/// The training edges that reproduce this baseline. At least one.
#[derive(Deserialize)]
struct Clients {
    #[serde(default)]
    python: Option<Edge>,
    #[serde(default)]
    rust: Option<Edge>,
}

#[derive(Deserialize)]
struct Edge {
    /// Python: the `e2e_*` example dir. Rust: the `conflux-client` example.
    harness: String,
    expected: f64,
    tolerance: f64,
    smoke: Cfg,
    #[serde(default)]
    full: Option<Cfg>,
}

#[derive(Deserialize, Clone, Copy)]
struct Cfg {
    clients: u32,
    rounds: u32,
}

#[derive(Clone, Copy, PartialEq)]
enum ClientKind {
    Python,
    Rust,
}

impl ClientKind {
    fn label(self) -> &'static str {
        match self {
            ClientKind::Python => "python",
            ClientKind::Rust => "rust",
        }
    }
}

fn main() {
    // Force the strategy families to link so the `inventory` registry is
    // populated — otherwise the `submit!` statics are dead-stripped and
    // every method looks "unknown" (the catalog example documents this).
    let _ = conflux_core::build_aggregator;
    let _ = conflux_selector::build_selector;
    let _ = conflux_privacy::build_privacy_mechanism;

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("list") => cmd_list(),
        Some("run") => cmd_run(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some("-h") | Some("--help") | None => usage(),
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            usage();
            exit(EXIT_FAIL);
        }
    }
}

fn usage() {
    println!(
        "conflux-baselines — reproduce published FL results\n\n\
         USAGE:\n\
         \x20 list                                    every discovered baseline\n\
         \x20 run <name> [--client python|rust] [--full] [--plan]\n\
         \x20 verify [--ci] [--plan]                  run every baseline's Rust edge\n\n\
         Edges: python drives the e2e_* PyTorch harness; rust drives the Burn\n\
         `conflux-client` example. --plan validates + prints the plan without running.\n\
         `verify` uses the Rust edge — fast, deterministic, no Python needed."
    );
}

// ---------------------------------------------------------------------------
// Discovery + loading
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate is two levels below the repo root")
        .to_path_buf()
}

fn baselines_dir() -> PathBuf {
    repo_root().join("baselines")
}

/// Every `baselines/<name>/baseline.toml`, sorted by name. `_`-prefixed
/// directories (e.g. `_harness`) are not baselines.
fn discover() -> Vec<(String, Manifest)> {
    let dir = baselines_dir();
    let read = std::fs::read_dir(&dir).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", dir.display());
        exit(EXIT_FAIL);
    });
    let mut out = Vec::new();
    for entry in read.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('_') {
            continue;
        }
        let toml_path = entry.path().join("baseline.toml");
        if !toml_path.exists() {
            continue;
        }
        out.push((name.clone(), load_manifest(&toml_path, &name)));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn load_manifest(path: &Path, name: &str) -> Manifest {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("baseline '{name}': cannot read {}: {e}", path.display());
        exit(EXIT_FAIL);
    });
    toml::from_str(&text).unwrap_or_else(|e| {
        eprintln!("baseline '{name}': invalid manifest: {e}");
        exit(EXIT_FAIL);
    })
}

fn load_one(name: &str) -> Manifest {
    let path = baselines_dir().join(name).join("baseline.toml");
    if !path.exists() {
        eprintln!("no baseline '{name}' (looked for {})", path.display());
        exit(EXIT_FAIL);
    }
    load_manifest(&path, name)
}

// ---------------------------------------------------------------------------
// Registry validation — a method must be in the catalog to be referenced.
// ---------------------------------------------------------------------------

fn method_exists(kind: StrategyKind, name: &str) -> bool {
    entries(kind).iter().any(|e| e.name == name)
}

fn validate_methods(m: &Method) -> Result<(), String> {
    if !method_exists(StrategyKind::Aggregator, &m.aggregator) {
        return Err(format!(
            "aggregator '{}' is not in the catalog",
            m.aggregator
        ));
    }
    if let Some(s) = &m.selector
        && !method_exists(StrategyKind::Selector, s)
    {
        return Err(format!("selector '{s}' is not in the catalog"));
    }
    if let Some(p) = &m.privacy_mechanism
        && !method_exists(StrategyKind::PrivacyMechanism, p)
    {
        return Err(format!("privacy_mechanism '{p}' is not in the catalog"));
    }
    Ok(())
}

fn edges(m: &Manifest) -> Vec<ClientKind> {
    let mut v = Vec::new();
    if m.clients.python.is_some() {
        v.push(ClientKind::Python);
    }
    if m.clients.rust.is_some() {
        v.push(ClientKind::Rust);
    }
    v
}

fn edge(m: &Manifest, kind: ClientKind) -> Option<&Edge> {
    match kind {
        ClientKind::Python => m.clients.python.as_ref(),
        ClientKind::Rust => m.clients.rust.as_ref(),
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn cmd_list() {
    let all = discover();
    if all.is_empty() {
        println!("no baselines found under {}", baselines_dir().display());
        return;
    }
    println!("{} baseline(s):\n", all.len());
    println!(
        "{:<24}{:<14}{:<14}{:<16}PAPER",
        "NAME", "METHOD", "EDGES", "SCENARIO"
    );
    for (name, m) in &all {
        let edge_list = edges(m)
            .iter()
            .map(|k| k.label())
            .collect::<Vec<_>>()
            .join(",");
        let scenario = if m.scenario.attackers > 0 {
            format!("{} attacker(s)", m.scenario.attackers)
        } else {
            "clean".to_string()
        };
        println!(
            "{:<24}{:<14}{:<14}{:<16}{}",
            name, m.method.aggregator, edge_list, scenario, m.paper.venue
        );
    }
    println!(
        "\nrun `... run <name> --client rust --plan` to validate a manifest and see its plan."
    );
}

// ---------------------------------------------------------------------------
// run
// ---------------------------------------------------------------------------

fn cmd_run(args: &[String]) {
    let mut name: Option<String> = None;
    let mut plan = false;
    let mut full = false;
    let mut client: Option<ClientKind> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--plan" => plan = true,
            "--full" => full = true,
            "--client" => {
                i += 1;
                client = match args.get(i).map(String::as_str) {
                    Some("python") => Some(ClientKind::Python),
                    Some("rust") => Some(ClientKind::Rust),
                    other => {
                        eprintln!("--client expects python|rust, got {other:?}");
                        exit(EXIT_FAIL);
                    }
                };
            }
            s if s.starts_with("--") => {
                eprintln!("unknown flag: {s}");
                exit(EXIT_FAIL);
            }
            s if name.is_none() => name = Some(s.to_string()),
            s => {
                eprintln!("unexpected argument: {s}");
                exit(EXIT_FAIL);
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        eprintln!("usage: run <name> [--client python|rust] [--full] [--plan]");
        exit(EXIT_FAIL);
    };

    let m = load_one(&name);
    if let Err(e) = validate_methods(&m.method) {
        eprintln!("✗ {name}: {e}");
        eprintln!("  (see `cargo run -p conflux-core --example catalog` for available methods)");
        exit(EXIT_UNKNOWN_METHOD);
    }

    // Chosen edge, or the manifest's default (Python first, else Rust).
    let kind = client.unwrap_or_else(|| {
        *edges(&m).first().unwrap_or_else(|| {
            eprintln!("✗ {name}: manifest declares no client edges");
            exit(EXIT_FAIL);
        })
    });
    let Some(e) = edge(&m, kind) else {
        eprintln!(
            "✗ {name}: no {} edge (available: {})",
            kind.label(),
            edges(&m)
                .iter()
                .map(|k| k.label())
                .collect::<Vec<_>>()
                .join(",")
        );
        exit(EXIT_FAIL);
    };
    let cfg = if full {
        e.full.unwrap_or(e.smoke)
    } else {
        e.smoke
    };

    print_plan(
        &name,
        &m,
        kind,
        e,
        &cfg,
        if full { "full" } else { "smoke" },
    );
    if plan {
        return;
    }

    match drive(&m, kind, e, &cfg) {
        Some(acc) => {
            let delta = (acc - e.expected).abs();
            println!(
                "\n  achieved {METRIC} = {acc:.4}   target {:.2} ± {:.2}   Δ={delta:.4}",
                e.expected, e.tolerance
            );
            if delta <= e.tolerance {
                println!("✓ {name} [{}]: REPRODUCED", kind.label());
            } else {
                println!("✗ {name} [{}]: MISS — outside tolerance", kind.label());
                exit(EXIT_FAIL);
            }
        }
        None => {
            eprintln!("✗ {name}: harness produced no `{METRIC}` (see its logs)");
            exit(EXIT_FAIL);
        }
    }
}

fn print_plan(name: &str, m: &Manifest, kind: ClientKind, e: &Edge, cfg: &Cfg, label: &str) {
    let sel = m.method.selector.as_deref().unwrap_or("—");
    let scenario = if m.scenario.attackers > 0 {
        format!(
            "{} attacker(s), no_reputation={}",
            m.scenario.attackers, m.scenario.no_reputation
        )
    } else {
        "clean (no attackers)".to_string()
    };
    println!("baseline: {name}");
    println!("  paper:    {}", m.paper.title);
    println!(
        "            {} · {} · {}",
        m.paper.authors, m.paper.venue, m.paper.url
    );
    println!(
        "  method:   aggregator={}  selector={sel}  ✓ validated against the catalog",
        m.method.aggregator
    );
    println!("  scenario: {scenario}");
    println!(
        "  edge:     {} — harness {} (dataset={}, model={}, partition={})",
        kind.label(),
        e.harness,
        m.experiment.dataset,
        m.experiment.model,
        m.experiment.partition
    );
    println!(
        "  config:   [{label}] clients={} rounds={}",
        cfg.clients, cfg.rounds
    );
    println!(
        "  target:   {METRIC} = {:.2} ± {:.2}",
        e.expected, e.tolerance
    );
    println!("  command:  {}", pretty_command(m, kind, e, cfg));
}

// ---------------------------------------------------------------------------
// verify — runs the Rust edge (fast, deterministic, no Python) for each
// baseline that has one. This is what a CI gate would run.
// ---------------------------------------------------------------------------

fn cmd_verify(args: &[String]) {
    let mut plan = false;
    for a in args {
        match a.as_str() {
            "--plan" => plan = true,
            "--ci" => {} // accepted; the Rust edge is already the CI-friendly one
            s => {
                eprintln!("unknown flag: {s}");
                exit(EXIT_FAIL);
            }
        }
    }
    let all = discover();
    println!(
        "verifying {} baseline(s) via the Rust (Burn) edge:\n",
        all.len()
    );

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    for (name, m) in &all {
        if let Err(err) = validate_methods(&m.method) {
            println!("✗ {name}: {err}");
            failed += 1;
            continue;
        }
        let Some(e) = edge(m, ClientKind::Rust) else {
            println!("– {name}: no rust edge, skipped");
            skipped += 1;
            continue;
        };
        let cfg = e.smoke;
        if plan {
            println!(
                "• {name}: [rust/smoke] {} clients={} rounds={} attackers={} → {METRIC} {:.2}±{:.2}",
                m.method.aggregator,
                cfg.clients,
                cfg.rounds,
                m.scenario.attackers,
                e.expected,
                e.tolerance
            );
            continue;
        }
        match drive(m, ClientKind::Rust, e, &cfg) {
            Some(acc) => {
                let ok = (acc - e.expected).abs() <= e.tolerance;
                println!(
                    "{} {name}: {acc:.4} (target {:.2}±{:.2})",
                    if ok { "✓" } else { "✗" },
                    e.expected,
                    e.tolerance
                );
                if ok {
                    passed += 1;
                } else {
                    failed += 1;
                }
            }
            None => {
                println!("✗ {name}: no metric produced");
                failed += 1;
            }
        }
    }

    if plan {
        return;
    }
    println!("\n{passed} passed, {failed} failed, {skipped} skipped");
    if failed > 0 {
        exit(EXIT_FAIL);
    }
}

// ---------------------------------------------------------------------------
// Harness orchestration
// ---------------------------------------------------------------------------

fn pretty_command(m: &Manifest, kind: ClientKind, e: &Edge, cfg: &Cfg) -> String {
    match kind {
        ClientKind::Python => {
            let mut s = format!(
                "bash python/conflux_client/examples/{}/run_demo.sh {} {} {}",
                e.harness, m.method.aggregator, cfg.clients, cfg.rounds
            );
            if m.scenario.attackers > 0 {
                s.push_str(" --poison");
            }
            if m.scenario.no_reputation {
                s.push_str(" --no-reputation");
            }
            s
        }
        ClientKind::Rust => format!(
            "cargo run --example {} -p conflux-client --features burn -- --aggregator {} --clients {} --rounds {} --attackers {}",
            e.harness, m.method.aggregator, cfg.clients, cfg.rounds, m.scenario.attackers
        ),
    }
}

fn drive(m: &Manifest, kind: ClientKind, e: &Edge, cfg: &Cfg) -> Option<f64> {
    match kind {
        ClientKind::Python => drive_python(m, e, cfg),
        ClientKind::Rust => drive_rust(m, e, cfg),
    }
}

fn drive_python(m: &Manifest, e: &Edge, cfg: &Cfg) -> Option<f64> {
    let example_dir = repo_root()
        .join("python/conflux_client/examples")
        .join(&e.harness);
    let script = example_dir.join("run_demo.sh");
    if !script.exists() {
        eprintln!("harness script not found: {}", script.display());
        exit(EXIT_FAIL);
    }
    let mut argv = vec![
        m.method.aggregator.clone(),
        cfg.clients.to_string(),
        cfg.rounds.to_string(),
    ];
    if m.scenario.attackers > 0 {
        argv.push("--poison".into());
    }
    if m.scenario.no_reputation {
        argv.push("--no-reputation".into());
    }
    println!("\n  driving the Python harness (real federation, needs the venv)…");
    let output = Command::new("bash")
        .arg(&script)
        .args(&argv)
        .current_dir(&example_dir)
        .output()
        .unwrap_or_else(|err| {
            eprintln!("failed to launch harness: {err}");
            exit(EXIT_FAIL);
        });
    parse_last_metric(&String::from_utf8_lossy(&output.stdout))
}

fn drive_rust(m: &Manifest, e: &Edge, cfg: &Cfg) -> Option<f64> {
    // Invoke the Burn example through cargo (debug — reuses the cached build).
    // `env!("CARGO")` is the same cargo running us, as the catalog golden test does.
    let cargo_args = [
        "run".to_string(),
        "--example".to_string(),
        e.harness.clone(),
        "-p".to_string(),
        "conflux-client".to_string(),
        "--features".to_string(),
        "burn".to_string(),
        "--".to_string(),
        "--aggregator".to_string(),
        m.method.aggregator.clone(),
        "--clients".to_string(),
        cfg.clients.to_string(),
        "--rounds".to_string(),
        cfg.rounds.to_string(),
        "--attackers".to_string(),
        m.scenario.attackers.to_string(),
    ];
    let output = Command::new(env!("CARGO"))
        .args(&cargo_args)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|err| {
            eprintln!("failed to launch the Burn example: {err}");
            exit(EXIT_FAIL);
        });
    parse_last_metric(&String::from_utf8_lossy(&output.stdout))
}

/// Both edges print `... held_out_accuracy=<number> ...`; the last one is
/// the final-round result.
fn parse_last_metric(s: &str) -> Option<f64> {
    let needle = format!("{METRIC}=");
    let mut last = None;
    for line in s.lines() {
        if let Some(pos) = line.find(&needle) {
            let rest = &line[pos + needle.len()..];
            let num: String = rest
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.')
                .collect();
            if let Ok(v) = num.parse::<f64>() {
                last = Some(v);
            }
        }
    }
    last
}
