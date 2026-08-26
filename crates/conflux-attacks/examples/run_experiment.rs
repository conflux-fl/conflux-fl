//! CLI experiment runner for `docs/research/temporal-consistency-aggregation.md`'s
//! Section 2 — runs one (aggregator × attack × collusion size × rounds)
//! configuration and prints one JSON line per round to stdout. Meant to
//! be driven by the shell scripts in `docs/research/scripts/`, which
//! sweep the actual parameter grid and collect every line into a single
//! JSONL results file for later analysis/plotting — this binary itself
//! only ever runs one configuration per invocation, deliberately, so a
//! single run is trivial to reproduce and debug in isolation.
//!
//! An `example`, not a separate workspace crate: this is a research/dev
//! tool, not a product component, and `conflux-attacks` already has
//! `conflux-core` as a dev-dependency for its own `tests/
//! attack_vs_defense.rs` (ADR 0010) — examples can use dev-dependencies
//! too, so this needed no new crate, no new workspace member, and no new
//! ADR to justify one. Run via `cargo run --release --example
//! run_experiment -p conflux-attacks -- --aggregator ... --attack ...`.
//!
//! One aggregator instance is built once and reused across every round
//! within a single invocation — required for `foolsgold`'s cross-round
//! history to behave as it would in a real deployment; harmless for
//! every stateless method, which ignores the reuse entirely.

use std::collections::HashMap;
use std::sync::Mutex;

use conflux_attacks::{
    AdaptiveEvasionAttack, AlieAttack, Attack, GaussianAttack, PersistentSybilAttack,
    RoundFeedback, ScalingAttack, SignFlippingAttack,
};
use conflux_core::{Aggregator, DssAggregator, build_aggregator};
use conflux_proto::{ClientDelta, decode_weights, encode_weights};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand_distr::{Distribution, Normal};
use serde::Serialize;

#[derive(Serialize)]
struct RoundResult {
    aggregator: String,
    attack: String,
    round: u64,
    num_honest: usize,
    num_attackers: usize,
    dim: usize,
    byzantine_fraction: f32,
    seed: u64,
    distance_from_true_value: f32,
    /// `null` when there were no attackers this round (nothing to
    /// measure "success" against) — see this file's `asr` doc comment.
    asr: Option<f32>,
}

struct Args {
    aggregator: String,
    attack: String,
    num_honest: usize,
    num_attackers: usize,
    dim: usize,
    seed: u64,
    byzantine_fraction: f32,
    rounds: u64,
    /// Every attack's own magnitude/direction knobs are fixed, documented
    /// defaults (matching `conflux-attacks`' own test conventions) rather
    /// than exposed as CLI flags — keeps the sweep grid
    /// (aggregator × attack × collusion size) the thing that varies,
    /// not a combinatorial explosion of per-attack tuning too. Revisit
    /// if a specific experiment needs to vary one.
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
    fn get_required(map: &HashMap<String, String>, key: &str) -> String {
        map.get(key)
            .unwrap_or_else(|| panic!("--{key} is required"))
            .clone()
    }

    Args {
        aggregator: get_required(&map, "aggregator"),
        attack: get_required(&map, "attack"),
        num_honest: get(&map, "num-honest", 8),
        num_attackers: get(&map, "num-attackers", 0),
        dim: get(&map, "dim", 3),
        seed: get(&map, "seed", 1),
        byzantine_fraction: get(&map, "byzantine-fraction", 0.2),
        rounds: get(&map, "rounds", 1),
        attack_magnitude: get(&map, "attack-magnitude", 50.0),
    }
}

fn honest_batch(n: usize, dim: usize, seed: u64) -> Vec<ClientDelta> {
    let mut rng = StdRng::seed_from_u64(seed);
    let noise = Normal::new(0.0, 0.3).unwrap();
    (0..n)
        .map(|i| {
            let weights: Vec<f32> = (0..dim)
                .map(|_| 1.0 + noise.sample(&mut rng) as f32)
                .collect();
            ClientDelta {
                client_id: format!("honest-{i}"),
                round: 1,
                weights: encode_weights(&weights),
                num_samples: 10,
            }
        })
        .collect()
}

/// Builds the requested attack, boxed uniformly so `main`'s round loop
/// can call `craft_adaptive` on any of them (stateless attacks simply
/// use the trait's default, which ignores the feedback argument).
fn build_attack(attack: &str, dim: usize, seed: u64, magnitude: f32) -> Box<dyn Attack> {
    match attack {
        "none" => Box::new(NoAttack),
        "gaussian" => Box::new(GaussianAttack {
            std_dev: magnitude,
            seed,
        }),
        "sign_flipping" => Box::new(SignFlippingAttack {
            scale: magnitude / 10.0,
        }),
        "scaling" => Box::new(ScalingAttack {
            scale_factor: magnitude / 10.0,
            malicious_direction: vec![magnitude * 2.0; dim],
        }),
        "alie" => Box::new(AlieAttack),
        "persistent_sybil" => Box::new(PersistentSybilAttack {
            fixed_update: vec![magnitude; dim],
        }),
        "adaptive_evasion" => Box::new(AdaptiveEvasionAttack::new(vec![1.0; dim], magnitude)),
        other => panic!(
            "unknown attack \"{other}\" (known: none, gaussian, sign_flipping, scaling, alie, \
             persistent_sybil, adaptive_evasion)"
        ),
    }
}

/// `DssAggregator` is deliberately not in `build_aggregator`'s
/// `inventory`-backed catalog (§6.2's doc comment: a research hypothesis,
/// never a framework default) — so a `--aggregator dss_<base>` name (e.g.
/// `dss_fedavg`, `dss_krum`) is handled here instead, wrapping whatever
/// `build_aggregator` constructs for `<base>` in a `DssAggregator`.
///
/// Two more prefixes build **ablated** variants of the same wrapper, for
/// Experiment 2.5 (`docs/research/temporal-consistency-aggregation.md`
/// §5.6 — the stability/collusion mechanism ablation §7.3 called for):
/// `dssstab_<base>` sets `collusion_threshold` below any real cosine
/// similarity (`-2.0`, since cosine ∈ [-1, 1]) so the "colluding" half of
/// the AND-gate is always true — a client is penalized on **stability
/// alone**. `dsscoll_<base>` sets `stability_threshold` above any real
/// stability score (`1.5`, since stability ∈ (0, 1]) so the "unstable"
/// half is always true — a client is penalized on **collusion alone**.
/// Both reuse `DssAggregator`'s already-`pub` threshold fields; no change
/// to `DssAggregator` itself was needed to add these two variants.
///
/// Anything without a `dss`-prefixed name is passed straight through to
/// `build_aggregator`.
fn build_experiment_aggregator(name: &str, byzantine_fraction: f32) -> Box<dyn Aggregator> {
    let build_base = |base_name: &str| {
        build_aggregator(base_name, byzantine_fraction).unwrap_or_else(|e| panic!("{e}"))
    };
    if let Some(base_name) = name.strip_prefix("dssstab_") {
        let mut dss = DssAggregator::new(build_base(base_name));
        dss.collusion_threshold = -2.0;
        return Box::new(dss);
    }
    if let Some(base_name) = name.strip_prefix("dsscoll_") {
        let mut dss = DssAggregator::new(build_base(base_name));
        dss.stability_threshold = 1.5;
        return Box::new(dss);
    }
    match name.strip_prefix("dss_") {
        Some(base_name) => Box::new(DssAggregator::new(build_base(base_name))),
        None => build_base(name),
    }
}

struct NoAttack;
impl Attack for NoAttack {
    fn craft(&self, _honest_updates: &[ClientDelta], _num_attackers: usize) -> Vec<ClientDelta> {
        Vec::new()
    }
}

fn distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
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

fn main() {
    let args = parse_args();
    let true_value = vec![1.0f32; args.dim];

    let aggregator = build_experiment_aggregator(&args.aggregator, args.byzantine_fraction);
    let attack = build_attack(&args.attack, args.dim, args.seed, args.attack_magnitude);
    let last_feedback: Mutex<Option<RoundFeedback>> = Mutex::new(None);

    for round in 0..args.rounds {
        let honest = honest_batch(args.num_honest, args.dim, args.seed + round);
        let feedback = last_feedback.lock().unwrap().clone();
        let attackers = attack.craft_adaptive(&honest, args.num_attackers, feedback.as_ref());

        // ASR's reference point, and (for adaptive attacks) next round's
        // feedback signal: how far the attackers' own average
        // submission sits from the truth — "how much they were reaching
        // for." `None` when there were no attackers this round; nothing
        // to measure attack success against, and nothing to give
        // feedback about either.
        let attacker_target = if attackers.is_empty() {
            None
        } else {
            Some(mean_vector(&attackers, args.dim))
        };
        let target_distance = attacker_target
            .as_ref()
            .map(|t| distance(t, &true_value).max(1e-6));

        let mut batch = honest;
        batch.extend(attackers.iter().cloned());
        let result = aggregator
            .aggregate(&batch)
            .unwrap_or_else(|e| panic!("round {round}: aggregation failed: {e}"));
        let distance_from_true_value = distance(&result, &true_value);

        *last_feedback.lock().unwrap() = attacker_target.map(|previous_submission| RoundFeedback {
            previous_submission,
            previous_aggregate: result.clone(),
        });

        let asr = target_distance.map(|td| distance_from_true_value / td);

        let row = RoundResult {
            aggregator: args.aggregator.clone(),
            attack: args.attack.clone(),
            round,
            num_honest: args.num_honest,
            num_attackers: args.num_attackers,
            dim: args.dim,
            byzantine_fraction: args.byzantine_fraction,
            seed: args.seed,
            distance_from_true_value,
            asr,
        };
        println!("{}", serde_json::to_string(&row).unwrap());
    }
}
