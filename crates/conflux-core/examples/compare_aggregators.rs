//! Runnable "try it" for `conflux-core`: every shipped aggregation
//! method, on one batch, so the differences between them are visible
//! rather than described.
//!
//! Run with:
//!   cargo run --example compare_aggregators -p conflux-core
//!
//! The batch is deliberately small and hand-readable — four honest
//! clients clustered around `[1, 1, 1]` and one attacker far away — so
//! you can check each method's answer against your own intuition instead
//! of trusting the program.

use conflux_core::{AggregatorError, AggregatorParams, build_aggregator};
use conflux_proto::{ClientDelta, encode_weights};

const ALL: &[&str] = &[
    "fedavg",
    "krum",
    "multi_krum",
    "trimmed_mean",
    "median",
    "faba",
    "bulyan",
    "geometric_median",
    "median_of_means",
    "divide_and_conquer",
    "foolsgold",
    "centered_clipping",
];

fn delta(id: &str, w: &[f32], num_samples: u64) -> ClientDelta {
    ClientDelta {
        client_id: id.to_string(),
        round: 1,
        weights: encode_weights(w),
        num_samples,
    }
}

fn run(label: &str, batch: &[ClientDelta], note: &str) {
    println!("\n=== {label} ===");
    println!("{note}\n");
    for &name in ALL {
        let aggregator = build_aggregator(name, AggregatorParams::default()).unwrap();
        match aggregator.aggregate(batch) {
            Ok(out) => println!("  {name:<20} {:?}", round3(&out)),
            Err(e) => println!("  {name:<20} rejected: {e}"),
        }
    }
}

fn round3(v: &[f32]) -> Vec<f32> {
    v.iter().map(|x| (x * 1000.0).round() / 1000.0).collect()
}

fn main() {
    // --- 1. No attacker. Every method should agree, roughly. ---------
    run(
        "clean batch",
        &[
            delta("a", &[1.0, 1.0, 1.0], 10),
            delta("b", &[1.1, 0.9, 1.0], 10),
            delta("c", &[0.9, 1.1, 1.0], 10),
            delta("d", &[1.0, 1.0, 1.1], 10),
        ],
        "Four honest clients around [1, 1, 1]. Robustness costs almost\n\
         nothing when there is nothing to defend against — every method\n\
         lands in the same place.",
    );

    // --- 2. One attacker, far away. ----------------------------------
    run(
        "one attacker",
        &[
            delta("a", &[1.0, 1.0, 1.0], 10),
            delta("b", &[1.1, 0.9, 1.0], 10),
            delta("c", &[0.9, 1.1, 1.0], 10),
            delta("attacker", &[50.0, 50.0, 50.0], 10),
        ],
        "Same honest cluster, plus one client submitting [50, 50, 50] —\n\
         and this is the case worth reading carefully, because most\n\
         methods here DO NOT defend it.\n\n\
         The default `byzantine_fraction` is 0.2, and 0.2 of four\n\
         clients rounds down to excluding *nobody*. So `multi_krum`,\n\
         `trimmed_mean`, `faba`, `bulyan`, `median_of_means`, and\n\
         `divide_and_conquer` all land on `fedavg`'s answer: their\n\
         robustness is parameterized, and the parameter says there is\n\
         nothing to exclude. Only `krum` (always selects exactly one\n\
         update), `median`, and `geometric_median` — the methods with no\n\
         such parameter — resist by construction.\n\n\
         This is not a bug. It is what a mis-set assumption looks like,\n\
         and it is why the next section exists.",
    );

    // --- 3. Input a hostile client can actually send. -----------------
    run(
        "a client submits NaN",
        &[
            delta("a", &[1.0, 1.0, 1.0], 10),
            delta("b", &[1.1, 0.9, 1.0], 10),
            delta("hostile", &[f32::NAN, f32::NAN, f32::NAN], 10),
        ],
        "Four bytes. Every method rejects the batch and names the client\n\
         and the coordinate — before this validation existed, six of\n\
         them panicked and took the server down, and the other six\n\
         returned NaN into the checkpoint.",
    );

    // --- 4. A lie the wire format cannot prevent. --------------------
    run(
        "a client claims u64::MAX samples",
        &[
            delta("a", &[1.0, 1.0, 1.0], 10),
            delta("b", &[1.1, 0.9, 1.0], 10),
            delta("liar", &[99.0, 99.0, 99.0], u64::MAX),
        ],
        "`num_samples` is self-reported. FedAvg weights by it, so before\n\
         this was bounded the liar's update *became* the aggregate\n\
         exactly, with every honest contribution numerically erased.",
    );

    // --- 5. What an aggregator's parameters actually change. ---------
    println!("\n=== the same batch, different byzantine_fraction ===");
    println!(
        "`byzantine_fraction` is an assumption about the batch, not a knob\n\
         to tune for output quality: it tells a method how many updates to\n\
         assume are malicious, and therefore how many to trim or exclude.\n\n\
         `trimmed_mean` shows it most directly — the fraction becomes a\n\
         literal count of values dropped from each end of each coordinate.\n\
         (`krum` would print the same answer at every setting here: it\n\
         always selects exactly one update, so with three honest clients\n\
         against two attackers it finds the honest cluster regardless.)\n"
    );
    let batch = [
        delta("a", &[1.0, 1.0, 1.0], 10),
        delta("b", &[1.1, 0.9, 1.0], 10),
        delta("c", &[0.9, 1.1, 1.0], 10),
        delta("attacker-1", &[50.0, 50.0, 50.0], 10),
        delta("attacker-2", &[51.0, 49.0, 50.0], 10),
    ];
    for fraction in [0.0, 0.2, 0.4] {
        let aggregator = build_aggregator(
            "trimmed_mean",
            AggregatorParams {
                byzantine_fraction: fraction,
                ..Default::default()
            },
        )
        .unwrap();
        let out = aggregator.aggregate(&batch).unwrap();
        println!("  byzantine_fraction = {fraction:<4} -> {:?}", round3(&out));
    }
    println!(
        "\n  Two of five clients are attacking. Assume none and the \
         attackers are simply averaged in; assume enough and they are \
         trimmed away."
    );

    // --- 6. Errors are values, not panics. ---------------------------
    println!("\n=== every failure is a typed error ===\n");
    let fedavg = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
    for (label, batch) in [
        ("empty batch", vec![]),
        (
            "mismatched dimensions",
            vec![delta("a", &[1.0, 2.0], 10), delta("b", &[1.0], 10)],
        ),
    ] {
        match fedavg.aggregate(&batch) {
            Ok(_) => println!("  {label:<24} unexpectedly succeeded"),
            Err(e @ AggregatorError::EmptyBatch) => println!("  {label:<24} {e}"),
            Err(e) => println!("  {label:<24} {e}"),
        }
    }
    println!(
        "\n  Rejection is a `Result` the caller handles. No aggregator\n\
         panics on client input — `tests/adversarial_input.rs` enforces\n\
         that against all twelve."
    );
}
