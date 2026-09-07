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

use std::collections::HashMap;
use std::collections::hash_map::Entry;
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
    /// Which repetition of this combination.
    seed: u32,
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
    /// Repetitions. Every combination runs once per seed, and the spread
    /// across them is the point: a single seed on this harness carries a
    /// run-to-run spread wide enough that two methods can trade places
    /// without either being better.
    pub(crate) seeds: Vec<u32>,
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
    for &seed in &grid.seeds {
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
                        seed,
                    });
                }
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
    // budget. The grid varies none of those *within* a seed — but the
    // seed itself draws the subsample, so `pooled.pt` differs between
    // seeds and the bar is computed once per seed rather than once per
    // sweep.
    let mut baselines: HashMap<u32, f64> = HashMap::new();
    let mut failures: Vec<String> = Vec::new();

    for (i, c) in combinations.iter().enumerate() {
        let label = format!(
            "aggregator={} split={}{} attack={} seed={}",
            c.aggregator,
            c.split,
            c.alpha.map(|a| format!(":{a}")).unwrap_or_default(),
            c.attack,
            c.seed
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
            seed: c.seed,
            // A sweep compares methods under one shared assumption, so
            // it takes the default rather than letting each method pick
            // the fraction that flatters it.
            byzantine_fraction: None,
        };

        let outcome = match federation::run(repo_root, &plan) {
            Ok(outcome) => outcome,
            Err(message) => {
                eprintln!("  failed: {message}");
                failures.push(label);
                continue;
            }
        };

        if let Entry::Vacant(slot) = baselines.entry(c.seed) {
            let steps = grid.rounds * LOCAL_STEPS_PER_ROUND;
            match federation::centralized_baseline(
                repo_root,
                &plan.recipe,
                &federation::work_dir(repo_root),
                steps,
            ) {
                Ok(accuracy) => {
                    println!("  centralized baseline ({steps} steps): {accuracy:.4}");
                    slot.insert(accuracy);
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
                "seed": c.seed,
                "round": round.round,
                "held_out_accuracy": round.accuracy,
                "held_out_loss": round.loss,
                "client_acc_min": round.client_acc_min,
                "client_acc_std": round.client_acc_std,
                "centralized_baseline_accuracy": baselines.get(&c.seed),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_split_parses_into_its_partition_and_parameter() {
        assert_eq!(parse_split("iid").unwrap(), ("iid", None));
        assert_eq!(
            parse_split("dirichlet:0.5").unwrap(),
            ("dirichlet", Some(0.5))
        );
    }

    #[test]
    fn an_unparseable_split_names_what_was_expected() {
        // These come from a command line, so the message has to be
        // readable by whoever typed it — not just non-zero.
        let e = parse_split("dirichlet").unwrap_err();
        assert!(e.contains("iid"), "{e}");
        assert!(e.contains("dirichlet:<alpha>"), "{e}");

        let e = parse_split("dirichlet:half").unwrap_err();
        assert!(e.contains("alpha"), "{e}");
    }

    #[test]
    fn a_dirichlet_alpha_of_zero_is_still_a_number() {
        // Degenerate, but the sweep's job is to run what it was asked
        // for; the harness decides whether the value is usable.
        assert_eq!(
            parse_split("dirichlet:0").unwrap(),
            ("dirichlet", Some(0.0))
        );
    }
}
