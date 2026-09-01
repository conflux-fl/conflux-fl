//! Experiment 2.3 (docs/research/temporal-consistency-aggregation.md,
//! §3.3/§7.1): non-IID fairness — does a client whose local distribution
//! genuinely differs from the population get systematically down-weighted
//! by a defended aggregator, with **zero attackers present**?
//!
//! Measurement design, chosen specifically to work uniformly across every
//! aggregator regardless of family shape (`UpdateFilter`'s own
//! `SelectionResult` only exists for selection-based methods; coordinate-
//! wise/whole-vector/stateful methods have no equivalent) — **leave-one-out
//! influence**: `‖A(batch) − A(batch ∖ {i})‖`, i.e. how much removing
//! client `i` actually changes the aggregate. Works identically for all
//! eleven methods, treating each as a black box.
//!
//! Non-IID is modeled here as a minority sub-group of honest clients
//! centered at a *shifted* true value relative to the majority — for
//! equal-covariance Gaussians (which is what `honest_batch` in
//! `run_experiment.rs` already draws), KL-divergence between the
//! minority's and majority's underlying distributions is proportional to
//! the squared shift magnitude, so "shift" is a principled, not
//! arbitrary, proxy for the divergence axis in Figure 2 and Claim 2 (§3.3)
//! — not a fabricated stand-in.
//!
//! # Multi-round measurement (`--rounds`, added 2026-09-01)
//!
//! This experiment was single-round from the start, which was correct
//! for the eleven stateless methods it was written against. It is
//! **structurally unable to measure a cross-round method**, and that
//! turned out to matter: `DssAggregator` returns a stability score of
//! `1.0` for every client whose deviation trace is shorter than two
//! entries, so in a single round its AND-gate can never fire and DSS
//! behaves exactly like whatever it wraps. Running Experiment 2.3 with a
//! `dss_` name would have produced its base method's numbers and looked
//! like a result.
//!
//! That is the real reason §6.5's fairness question — "does dropping the
//! stability conjunct reopen Claim 2?" — stayed open across three
//! sessions. Not that the measurement was hard, but that the harness
//! could neither name a DSS variant (it called `build_aggregator`
//! directly) nor exercise one.
//!
//! With `--rounds N > 1`, leave-one-out influence becomes
//! `‖A_N(full) − A_N(full ∖ {i})‖`: both arms run `N` rounds against the
//! same per-round seed sequence, with **one** aggregator instance per arm
//! so cross-round state accumulates. The question it answers is the same
//! one, asked over an experiment rather than a round — how much did
//! client `i`'s presence throughout change where the model ended up?
//!
//! `--rounds 1` is the default and reproduces the original behavior
//! exactly, so Experiment 2.3's existing results remain valid and
//! re-runnable.
//!
//! Run via `cargo run --release --example run_fairness_experiment -p
//! conflux-attacks -- --aggregator ... --shift ...`.

use std::collections::HashMap;

mod common;
use common::build_experiment_aggregator;
use conflux_proto::{ClientDelta, encode_weights};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use serde::Serialize;

#[derive(Serialize)]
struct ClientResult {
    aggregator: String,
    shift: f32,
    seed: u64,
    rounds: usize,
    client_group: String, // "majority" | "minority"
    client_index: usize,
    /// `‖A(batch) − A(batch ∖ {client_index})‖` — how much removing this
    /// client actually changes the aggregate. This is the fairness
    /// metric: compare majority vs. minority influence at a given shift.
    leave_one_out_influence: f32,
}

struct Args {
    aggregator: String,
    num_majority: usize,
    num_minority: usize,
    shift: f32,
    seed: u64,
    dim: usize,
    byzantine_fraction: f32,
    clip_radius: f32,
    rounds: usize,
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
    fn get_required(map: &HashMap<String, String>, key: &str) -> String {
        map.get(key)
            .unwrap_or_else(|| panic!("--{key} is required"))
            .clone()
    }

    Args {
        aggregator: get_required(&map, "aggregator"),
        num_majority: get(&map, "num-majority", 6),
        num_minority: get(&map, "num-minority", 2),
        shift: get(&map, "shift", 0.0),
        seed: get(&map, "seed", 1),
        dim: get(&map, "dim", 3),
        byzantine_fraction: get(&map, "byzantine-fraction", 0.2),
        clip_radius: get(&map, "clip-radius", 1.0),
        // Default 1: the original single-round behavior, so every
        // existing Experiment 2.3 result stays reproducible.
        rounds: get(&map, "rounds", 1),
    }
}

/// Majority centered at [1,1,...,1]; minority centered at
/// [1+shift, 1, ..., 1] — shifted along one axis only, so `shift` alone
/// (not a multi-dimensional direction choice) controls the divergence.
fn build_batch(args: &Args, seed: u64) -> (Vec<ClientDelta>, Vec<(String, usize)>) {
    let mut rng = StdRng::seed_from_u64(seed);
    let noise = Normal::new(0.0, 0.3).unwrap();
    let mut deltas = Vec::new();
    let mut groups = Vec::new();

    for i in 0..args.num_majority {
        let weights: Vec<f32> = (0..args.dim)
            .map(|_| 1.0 + noise.sample(&mut rng) as f32)
            .collect();
        deltas.push(ClientDelta {
            client_id: format!("majority-{i}"),
            round: 1,
            weights: encode_weights(&weights),
            num_samples: 10,
            ..Default::default()
        });
        groups.push(("majority".to_string(), i));
    }
    for i in 0..args.num_minority {
        let weights: Vec<f32> = (0..args.dim)
            .map(|d| {
                let base = if d == 0 { 1.0 + args.shift } else { 1.0 };
                base + noise.sample(&mut rng) as f32
            })
            .collect();
        deltas.push(ClientDelta {
            client_id: format!("minority-{i}"),
            round: 1,
            weights: encode_weights(&weights),
            num_samples: 10,
            ..Default::default()
        });
        groups.push(("minority".to_string(), i));
    }
    (deltas, groups)
}

fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Runs `rounds` rounds against one aggregator instance and returns the
/// final aggregate.
///
/// One instance for the whole run, not one per round: that is the entire
/// point of the multi-round mode. A fresh aggregator each round would
/// reset the cross-round state being measured and silently reproduce the
/// single-round result.
///
/// `skip` removes one client from every round, which is the leave-one-out
/// arm. Removing it from *every* round rather than just the last matters
/// for a stateful method — a client that was present for nineteen rounds
/// has already shaped the history the twentieth is judged against.
fn run_arm(args: &Args, skip: Option<usize>) -> Vec<f32> {
    let aggregator =
        build_experiment_aggregator(&args.aggregator, args.byzantine_fraction, args.clip_radius);

    let mut last = Vec::new();
    for round in 0..args.rounds {
        // Same per-round seed sequence in both arms, so the two differ
        // only by the removed client and not by the noise draw.
        let (mut batch, _) = build_batch(args, args.seed + round as u64);
        if let Some(index) = skip {
            batch.remove(index);
        }
        last = aggregator
            .aggregate(&batch)
            .unwrap_or_else(|e| panic!("aggregation failed in round {round}: {e}"));
    }
    last
}

fn main() {
    let args = parse_args();
    // Group labels come from a round-0 batch; membership is identical in
    // every round, only the noise differs.
    let (_, groups) = build_batch(&args, args.seed);

    let full_result = run_arm(&args, None);

    for (idx, (group, group_idx)) in groups.iter().enumerate() {
        let result_without_i = run_arm(&args, Some(idx));

        let row = ClientResult {
            aggregator: args.aggregator.clone(),
            shift: args.shift,
            seed: args.seed,
            rounds: args.rounds,
            client_group: group.clone(),
            client_index: *group_idx,
            leave_one_out_influence: distance(&full_result, &result_without_i),
        };
        println!("{}", serde_json::to_string(&row).unwrap());
    }
}
