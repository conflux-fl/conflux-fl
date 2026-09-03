//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-attacks`](https://confluxfl.dev/crate-deep-dives/conflux-attacks/):
//! every shipped attack against every shipped defense, on the same batch.
//!
//! Run with:
//!   cargo run --release --example attack_vs_defense -p conflux-attacks
//!
//! One table, one glance, no output files — the quickest way to see
//! which defenses hold against which attacks.
//!
//! **Test/dev only.** `conflux-server` must never depend on this crate,
//! at any depth.

use conflux_attacks::{
    AdaptiveEvasionAttack, AlieAttack, Attack, CorrelatedSybilAttack, GaussianAttack,
    PersistentSybilAttack, ScalingAttack, SignFlippingAttack,
};
use conflux_core::{AggregatorParams, build_aggregator};
use conflux_proto::{ClientDelta, decode_weights, encode_weights};

const DIM: usize = 3;
const HONEST: usize = 8;
const ATTACKERS: usize = 2;

fn honest_batch() -> Vec<ClientDelta> {
    (0..HONEST)
        .map(|i| {
            // Deterministic, mildly heterogeneous honest clients around 1.0.
            let jitter = (i as f32 - HONEST as f32 / 2.0) * 0.02;
            ClientDelta {
                client_id: format!("honest-{i}"),
                round: 1,
                weights: encode_weights(&[1.0 + jitter; DIM]),
                num_samples: 10,
                ..Default::default()
            }
        })
        .collect()
}

fn attacks() -> Vec<(&'static str, Box<dyn Attack>)> {
    vec![
        (
            "gaussian",
            Box::new(GaussianAttack {
                std_dev: 50.0,
                seed: 1,
            }),
        ),
        ("sign_flipping", Box::new(SignFlippingAttack { scale: 5.0 })),
        (
            "scaling",
            Box::new(ScalingAttack {
                scale_factor: 5.0,
                malicious_direction: vec![100.0; DIM],
            }),
        ),
        ("alie", Box::new(AlieAttack)),
        (
            "persistent_sybil",
            Box::new(PersistentSybilAttack {
                fixed_update: vec![50.0; DIM],
            }),
        ),
        (
            "correlated_sybil",
            Box::new(CorrelatedSybilAttack {
                shared_update: vec![50.0; DIM],
                divergence: 10.0,
                resample_each_round: false,
                seed: 1,
            }),
        ),
        (
            "adaptive_evasion",
            Box::new(AdaptiveEvasionAttack::new(vec![1.0; DIM], 50.0)),
        ),
    ]
}

const DEFENSES: &[&str] = &[
    "fedavg",
    "krum",
    "multi_krum",
    "trimmed_mean",
    "median",
    "geometric_median",
    "foolsgold",
];

/// Distance from the honest consensus of 1.0 — lower is better.
fn error_from_truth(weights: &[f32]) -> f32 {
    weights
        .iter()
        .map(|w| (w - 1.0).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn main() {
    println!(
        "Distance from the honest consensus [1, 1, 1] after one round.\n\
         Lower is better; `fedavg` is the undefended control.\n\
         {HONEST} honest clients, {ATTACKERS} attackers, byzantine_fraction = 0.2.\n"
    );

    print!("{:<20}", "attack");
    for d in DEFENSES {
        print!("{d:>18}");
    }
    println!();
    println!("{}", "-".repeat(20 + 18 * DEFENSES.len()));

    for (name, attack) in attacks() {
        let mut batch = honest_batch();
        batch.extend(attack.craft(&honest_batch(), ATTACKERS));

        print!("{name:<20}");
        for &defense in DEFENSES {
            let aggregator = build_aggregator(
                defense,
                AggregatorParams {
                    byzantine_fraction: 0.2,
                    ..Default::default()
                },
            )
            .unwrap();
            match aggregator.aggregate(&batch) {
                Ok(out) => print!("{:>18.3}", error_from_truth(&out)),
                Err(_) => print!("{:>18}", "rejected"),
            }
        }
        println!();
    }

    println!(
        "\nTwo things worth reading off this table.\n\n\
         `fedavg`'s column is the cost of no defense — and note it is not\n\
         uniformly catastrophic. Against `alie`, an attack specifically\n\
         designed to stay inside the honest variance, it barely moves,\n\
         which is the point of that attack.\n\n\
         `foolsgold`'s column is roughly flat and never near zero. It\n\
         detects collusion rather than outliers, so it neither collapses\n\
         under a scaling attack nor tracks the honest consensus closely —\n\
         a different objective, not a worse one."
    );

    // The reassembly step the server does before any of this runs.
    let sample = honest_batch();
    let decoded = decode_weights(&sample[0].weights).unwrap();
    println!(
        "\n(Each update is {} f32s on the wire: {decoded:?})",
        decoded.len()
    );
}
