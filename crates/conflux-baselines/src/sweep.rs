//! Sweeping a grid of (aggregator, split, attack) combinations.
//!
//! A baseline answers "does this method reproduce its paper's number?".
//! A sweep answers the comparative question next to it: given the same
//! model, data and attack, how do the methods differ from each other —
//! and does a difference that shows up on synthetic vectors survive
//! contact with a real model and a real dataset?
//!
//! Every combination is a real federation, not a simulation: real
//! training, real gRPC, real aggregation. That is the expensive choice
//! and the whole point, since the cheap version is what a sweep exists
//! to check.
//!
//! Output is JSONL, one record per round per combination, appended
//! rather than overwritten so a grid can be extended across several
//! invocations. `summarize_sweep.py` turns it into CSV and
//! `plot_sweep.py` into figures.

use std::io::Write;
use std::path::Path;

use crate::federation::{self, LOCAL_STEPS_PER_ROUND, Plan, Recipe};

/// One point of the grid.
struct Combination<'a> {
    aggregator: &'a str,
    /// `"iid"` or `"dirichlet"`.
    split: &'a str,
    /// Set only for `dirichlet`; lower is more skewed.
    alpha: Option<f64>,
    /// `"none"` or `"poison"`.
    attack: &'a str,
}

/// What the grid is swept over.
pub(crate) struct Grid {
    pub(crate) dataset: String,
    pub(crate) model: String,
    pub(crate) aggregators: Vec<String>,
    /// Each entry is `iid` or `dirichlet:<alpha>`.
    pub(crate) splits: Vec<String>,
    /// Each entry is `none` or `poison`.
    pub(crate) attacks: Vec<String>,
    pub(crate) clients: u32,
    pub(crate) rounds: u32,
    pub(crate) out: String,
}

/// `iid` or `dirichlet:<alpha>` as the partition name plus its parameter.
fn parse_split(spec: &str) -> Result<(&str, Option<f64>), String> {
    if spec == "iid" {
        return Ok(("iid", None));
    }
    if let Some(alpha) = spec.strip_prefix("dirichlet:") {
        let alpha: f64 = alpha
            .parse()
            .map_err(|_| format!("{spec:?} has an unparseable alpha"))?;
        return Ok(("dirichlet", Some(alpha)));
    }
    Err(format!(
        "unrecognized split {spec:?} (expected 'iid' or 'dirichlet:<alpha>')"
    ))
}

/// Runs every combination, appending a record per round to `grid.out`.
///
/// One failed combination does not end the sweep: a grid is long, and
/// losing an hour of completed work because the twelfth of twenty
/// combinations hit a busy port is the wrong trade. Failures are counted
/// and named at the end.
pub(crate) fn run(repo_root: &Path, grid: &Grid) -> Result<(), String> {
    let mut combinations = Vec::new();
    for attack in &grid.attacks {
        if attack != "none" && attack != "poison" {
            return Err(format!(
                "unrecognized attack {attack:?} (expected 'none' or 'poison')"
            ));
        }
        for split_spec in &grid.splits {
            let (split, alpha) = parse_split(split_spec)?;
            for aggregator in &grid.aggregators {
                combinations.push(Combination {
                    aggregator,
                    split,
                    alpha,
                    attack,
                });
            }
        }
    }

    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&grid.out)
        .map_err(|e| format!("cannot open {}: {e}", grid.out))?;

    let total = combinations.len();
    // The centralized bar depends on the model, the data and the step
    // budget — none of which the grid varies — so it is computed once,
    // on the first combination that gets far enough to write `pooled.pt`.
    let mut baseline: Option<f64> = None;
    let mut failures: Vec<String> = Vec::new();

    for (i, c) in combinations.iter().enumerate() {
        let label = format!(
            "aggregator={} split={}{} attack={}",
            c.aggregator,
            c.split,
            c.alpha.map(|a| format!(":{a}")).unwrap_or_default(),
            c.attack
        );
        println!("\n[{}/{total}] {label}", i + 1);

        let poisoned = c.attack == "poison";
        let plan = Plan {
            recipe: Recipe {
                model: &grid.model,
                dataset: &grid.dataset,
                partition: c.split,
                dirichlet_alpha: c.alpha,
            },
            aggregator: c.aggregator,
            clients: grid.clients,
            // One Byzantine client, and the reputation filter off with
            // it: otherwise a separate defense may catch the attacker
            // first and every aggregator scores the same, which measures
            // the filter rather than the method.
            attackers: if poisoned { 1 } else { 0 },
            rounds: grid.rounds,
            no_reputation: poisoned,
        };

        let outcome = match federation::run(repo_root, &plan) {
            Ok(outcome) => outcome,
            Err(message) => {
                eprintln!("  failed: {message}");
                failures.push(label);
                continue;
            }
        };

        if baseline.is_none() {
            let steps = grid.rounds * LOCAL_STEPS_PER_ROUND;
            match federation::centralized_baseline(
                repo_root,
                &plan.recipe,
                &federation::work_dir(repo_root),
                steps,
            ) {
                Ok(accuracy) => {
                    println!("  centralized baseline ({steps} steps): {accuracy:.4}");
                    baseline = Some(accuracy);
                }
                // Not fatal: the federated numbers are still worth
                // recording, and the field is nullable in the schema.
                Err(message) => eprintln!("  no centralized baseline: {message}"),
            }
        }

        for round in &outcome.rounds {
            let record = serde_json::json!({
                "dataset": grid.dataset,
                "aggregator": c.aggregator,
                "split": c.split,
                "dirichlet_alpha": c.alpha,
                "attack": c.attack,
                "n_clients": grid.clients,
                "round": round.round,
                "held_out_accuracy": round.accuracy,
                "held_out_loss": round.loss,
                "centralized_baseline_accuracy": baseline,
            });
            writeln!(out, "{record}").map_err(|e| format!("cannot write to {}: {e}", grid.out))?;
        }
        out.flush()
            .map_err(|e| format!("cannot flush {}: {e}", grid.out))?;
    }

    println!(
        "\nwrote {} of {total} combinations to {}",
        total - failures.len(),
        grid.out
    );
    if !failures.is_empty() {
        return Err(format!(
            "{} combination(s) failed:\n  {}",
            failures.len(),
            failures.join("\n  ")
        ));
    }
    Ok(())
}
