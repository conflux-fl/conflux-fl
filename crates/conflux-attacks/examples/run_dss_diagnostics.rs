//! Per-client DSS diagnostics runner for `docs/research/
//! temporal-consistency-aggregation.md`'s §5.7 (the solo-attacker /
//! collusion-signal-provenance investigation) and §5.8 (the joint
//! non-IID + attack experiment). Unlike `run_experiment.rs`, which only
//! ever sees a `Box<dyn Aggregator>` (so it can sweep any shipped name,
//! but can't read DSS-specific internals through that trait object),
//! this runner holds a concrete `DssAggregator` directly and prints one
//! JSON line per (round, client) with that client's own
//! `stability`/`collusion`/`weight` from `DssAggregator::
//! last_diagnostics()` — the instrumentation added specifically so this
//! kind of per-client inspection doesn't require leave-one-out
//! re-aggregation (which would corrupt a stateful aggregator's own
//! history the moment a client is counterfactually dropped).
//!
//! Two scenarios, selected by `--scenario`:
//! - `attack`: honest batch + attackers (same shape as
//!   `run_experiment.rs`), for inspecting *why* DSS did or didn't
//!   penalize the attacker(s) in a given round.
//! - `joint`: a non-IID minority (shifted mean, independently noisy every
//!   round — not just a fixed shift) alongside the majority and
//!   attackers together, for the still-open "does DSS's fairness
//!   protection hold up under simultaneous attack" question (§5.4/§6.4's
//!   scope note).

use std::collections::HashMap;
use std::sync::Mutex;

use conflux_attacks::{AdaptiveEvasionAttack, Attack, PersistentSybilAttack, RoundFeedback};
use conflux_core::{Aggregator, AggregatorParams, DssAggregator, build_aggregator};
use conflux_proto::{ClientDelta, decode_weights, encode_weights};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use serde::Serialize;

#[derive(Serialize)]
struct ClientRow {
    scenario: String,
    base_aggregator: String,
    round: u64,
    seed: u64,
    /// Duplicated on every client row for this round so a single JSONL
    /// file lets per-client weights and the round's actual aggregation
    /// outcome be correlated without a second run or a second file —
    /// exactly the mismatch that made an earlier hand-analysis pass
    /// during this investigation briefly compare two *different*
    /// non-reproducing runs against each other by accident.
    distance_from_true_value: f32,
    client_id: String,
    client_role: String, // "majority" | "minority" | "attacker"
    stability: f32,
    collusion: f32,
    weight: f32,
}

struct Args {
    scenario: String,
    base_aggregator: String,
    attack: String,
    num_majority: usize,
    num_minority: usize,
    num_attackers: usize,
    minority_shift: f32,
    dim: usize,
    seed: u64,
    rounds: u64,
    attack_magnitude: f32,
}

fn parse_args() -> Args {
    let mut map: HashMap<String, String> = HashMap::new();
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let key = flag.trim_start_matches("--").to_string();
        let value = it
            .next()
            .unwrap_or_else(|| panic!("--{key} is missing its value"));
        map.insert(key, value);
    }
    fn get<T: std::str::FromStr>(map: &HashMap<String, String>, key: &str, default: T) -> T {
        map.get(key)
            .map(|v| {
                v.parse()
                    .unwrap_or_else(|_| panic!("--{key}={v:?} is not a valid value"))
            })
            .unwrap_or(default)
    }
    Args {
        scenario: get(&map, "scenario", "attack".to_string()),
        base_aggregator: get(&map, "base-aggregator", "fedavg".to_string()),
        attack: get(&map, "attack", "persistent_sybil".to_string()),
        num_majority: get(&map, "num-majority", 8),
        num_minority: get(&map, "num-minority", 0),
        num_attackers: get(&map, "num-attackers", 2),
        minority_shift: get(&map, "minority-shift", 3.0),
        dim: get(&map, "dim", 3),
        seed: get(&map, "seed", 1),
        rounds: get(&map, "rounds", 20),
        attack_magnitude: get(&map, "attack-magnitude", 50.0),
    }
}

fn make_delta(client_id: String, round: u64, num_samples: u64, weights: &[f32]) -> ClientDelta {
    ClientDelta {
        client_id,
        round,
        weights: encode_weights(weights),
        num_samples,
    }
}

/// Majority clients: mean 1.0, small iid noise every round — the plain
/// honest baseline every other experiment in this file already uses.
/// Reseeded fresh each round from `seed + round`, matching
/// `run_experiment.rs`'s `honest_batch` exactly — needed so a given
/// `--seed` produces the *same* honest batch here as it would in
/// `run_experiment.rs`, letting this tool's per-client diagnostics and
/// that tool's aggregate distance-from-truth be compared directly for
/// the identical underlying run, not two independently-seeded ones.
fn majority_batch(n: usize, dim: usize, seed: u64) -> Vec<ClientDelta> {
    let mut rng = StdRng::seed_from_u64(seed);
    let noise = Normal::new(0.0, 0.3).unwrap();
    (0..n)
        .map(|i| {
            let weights: Vec<f32> = (0..dim)
                .map(|_| 1.0 + noise.sample(&mut rng) as f32)
                .collect();
            make_delta(format!("majority-{i}"), 1, 10, &weights)
        })
        .collect()
}

/// Minority clients: mean `1.0 + shift`, *independently noisy every
/// round* (not a fixed shift repeated identically) — deliberately
/// different from Experiment 2.3's single-round leave-one-out design,
/// which used a fixed shift with no round-to-round variance at all.
/// Real non-IID clients don't submit an identical update every round;
/// giving the minority genuine round-to-round variance is what makes
/// this a fair test of whether DSS's *stability* signal (not just its
/// mean-shift tolerance) can be fooled into flagging an honest client —
/// the false-positive risk §6.2 step 5's AND-gate was specifically
/// designed to avoid.
fn minority_batch(n: usize, dim: usize, shift: f32, seed: u64) -> Vec<ClientDelta> {
    // A distinct RNG stream from `majority_batch`'s (offset seed), so
    // adding a minority never perturbs the majority's own values for a
    // given `--seed` — keeps `attack`-scenario and `joint`-scenario runs
    // with the same seed sharing an identical majority batch.
    let mut rng = StdRng::seed_from_u64(seed.wrapping_add(1_000_000));
    let noise = Normal::new(0.0, 0.3).unwrap();
    (0..n)
        .map(|i| {
            let weights: Vec<f32> = (0..dim)
                .map(|_| 1.0 + shift + noise.sample(&mut rng) as f32)
                .collect();
            make_delta(format!("minority-{i}"), 1, 10, &weights)
        })
        .collect()
}

fn mean_vector(deltas: &[ClientDelta], dim: usize) -> Vec<f32> {
    let mut mean = vec![0.0f32; dim];
    for d in deltas {
        let w = decode_weights(&d.weights).expect("crafted attack weights always decode");
        for (m, x) in mean.iter_mut().zip(&w) {
            *m += x;
        }
    }
    if !deltas.is_empty() {
        for m in &mut mean {
            *m /= deltas.len() as f32;
        }
    }
    mean
}

fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

fn build_attack(name: &str, dim: usize, magnitude: f32) -> Box<dyn Attack> {
    match name {
        "persistent_sybil" => Box::new(PersistentSybilAttack {
            fixed_update: vec![magnitude; dim],
        }),
        "adaptive_evasion" => Box::new(AdaptiveEvasionAttack::new(vec![1.0; dim], magnitude)),
        other => panic!("unknown attack \"{other}\" (known: persistent_sybil, adaptive_evasion)"),
    }
}

fn main() {
    let args = parse_args();
    let true_value = vec![1.0f32; args.dim];

    let base = build_aggregator(&args.base_aggregator, AggregatorParams::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let dss = DssAggregator::new(base);
    let attack = build_attack(&args.attack, args.dim, args.attack_magnitude);
    let last_feedback: Mutex<Option<RoundFeedback>> = Mutex::new(None);

    for round in 0..args.rounds {
        let mut batch = majority_batch(args.num_majority, args.dim, args.seed + round);
        let majority_ids: Vec<String> = batch.iter().map(|c| c.client_id.clone()).collect();

        let minority_ids: Vec<String> = if args.scenario == "joint" {
            let minority = minority_batch(
                args.num_minority,
                args.dim,
                args.minority_shift,
                args.seed + round,
            );
            let ids = minority.iter().map(|c| c.client_id.clone()).collect();
            batch.extend(minority);
            ids
        } else {
            Vec::new()
        };

        let honest_only = batch.clone();
        let feedback = last_feedback.lock().unwrap().clone();
        let attackers = if args.num_attackers > 0 {
            attack.craft_adaptive(&honest_only, args.num_attackers, feedback.as_ref())
        } else {
            Vec::new()
        };
        let attacker_ids: Vec<String> = attackers.iter().map(|c| c.client_id.clone()).collect();
        batch.extend(attackers.iter().cloned());

        let result = dss
            .aggregate(&batch)
            .unwrap_or_else(|e| panic!("round {round}: aggregation failed: {e}"));
        let distance_from_true_value = distance(&result, &true_value);

        *last_feedback.lock().unwrap() = if attackers.is_empty() {
            None
        } else {
            Some(RoundFeedback {
                previous_submission: mean_vector(&attackers, args.dim),
                previous_aggregate: result,
            })
        };

        for d in dss.last_diagnostics() {
            let role = if attacker_ids.contains(&d.client_id) {
                "attacker"
            } else if minority_ids.contains(&d.client_id) {
                "minority"
            } else if majority_ids.contains(&d.client_id) {
                "majority"
            } else {
                "unknown"
            };
            let row = ClientRow {
                scenario: args.scenario.clone(),
                base_aggregator: args.base_aggregator.clone(),
                round,
                seed: args.seed,
                distance_from_true_value,
                client_id: d.client_id,
                client_role: role.to_string(),
                stability: d.stability,
                collusion: d.collusion,
                weight: d.weight,
            };
            println!("{}", serde_json::to_string(&row).unwrap());
        }
    }
}
