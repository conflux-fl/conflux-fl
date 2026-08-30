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
//! Run via `cargo run --release --example run_fairness_experiment -p
//! conflux-attacks -- --aggregator ... --shift ...`.

use std::collections::HashMap;

use conflux_core::{AggregatorParams, build_aggregator};
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
    }
}

/// Majority centered at [1,1,...,1]; minority centered at
/// [1+shift, 1, ..., 1] — shifted along one axis only, so `shift` alone
/// (not a multi-dimensional direction choice) controls the divergence.
fn build_batch(args: &Args) -> (Vec<ClientDelta>, Vec<(String, usize)>) {
    let mut rng = StdRng::seed_from_u64(args.seed);
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

fn main() {
    let args = parse_args();
    let (batch, groups) = build_batch(&args);

    // Fresh aggregator per call — this experiment is single-round
    // (matching Experiment 2.1's design), so FoolsGold's cross-round
    // history isn't exercised here; a temporal fairness measurement is
    // separate future work (see this file's own module doc comment).
    let full_result = build_aggregator(
        &args.aggregator,
        AggregatorParams {
            byzantine_fraction: args.byzantine_fraction,
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"))
    .aggregate(&batch)
    .unwrap_or_else(|e| panic!("full-batch aggregation failed: {e}"));

    for (idx, (group, group_idx)) in groups.iter().enumerate() {
        let mut without_i = batch.clone();
        without_i.remove(idx);
        let result_without_i = build_aggregator(
            &args.aggregator,
            AggregatorParams {
                byzantine_fraction: args.byzantine_fraction,
                ..Default::default()
            },
        )
        .unwrap_or_else(|e| panic!("{e}"))
        .aggregate(&without_i)
        .unwrap_or_else(|e| panic!("leave-one-out aggregation failed: {e}"));

        let row = ClientResult {
            aggregator: args.aggregator.clone(),
            shift: args.shift,
            seed: args.seed,
            client_group: group.clone(),
            client_index: *group_idx,
            leave_one_out_influence: distance(&full_result, &result_without_i),
        };
        println!("{}", serde_json::to_string(&row).unwrap());
    }
}
