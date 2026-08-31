//! Every shipped aggregator, against input a hostile client can actually
//! send.
//!
//! This suite exists because two remotely-triggerable defects survived
//! 343 passing tests. Both were the same mistake: the aggregators were
//! tested against *plausible* batches, and a `ClientDelta` arrives from
//! the network, where nothing constrains it to be plausible.
//! `decode_weights` accepts any 4-byte pattern (it must — the codec
//! cannot know which bit patterns are meaningful for a given model), and
//! `num_samples` is an unauthenticated `u64` a client picks for itself.
//!
//! What was found, before the validation this file now guards:
//!
//! - **`NaN` in one client's weights**: six aggregators *panicked* —
//!   `krum`, `multi_krum`, `trimmed_mean`, `median`, `bulyan`,
//!   `median_of_means`, all via `partial_cmp(...).expect("never NaN")`,
//!   which took the server down. The other six returned `NaN`, which is
//!   worse in a slower way: it lands in the checkpoint and every
//!   subsequent round starts from it, so the experiment is over and
//!   nothing reports an error.
//! - **`num_samples: u64::MAX`**: FedAvg's output became exactly the
//!   liar's submission. Not "influenced by" — every honest contribution
//!   underflowed to nothing.
//!
//! The rule these tests encode: **an aggregator may reject a batch, and
//! may return a defensible number, but must never panic and must never
//! return a non-finite value.** Rejection is a `Result`, which callers
//! handle; a panic is a denial of service, and a `NaN` is silent
//! corruption.

use conflux_core::{AggregatorError, AggregatorParams, build_aggregator};
use conflux_proto::{ClientDelta, encode_weights};

/// Every name in `build_aggregator`'s catalog. Deliberately spelled out
/// rather than derived, so adding a method to the catalog without adding
/// it here is a visible omission in a diff.
const ALL_AGGREGATORS: &[&str] = &[
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

fn delta(client_id: &str, weights: &[f32], num_samples: u64) -> ClientDelta {
    ClientDelta {
        client_id: client_id.to_string(),
        round: 1,
        weights: encode_weights(weights),
        num_samples,
    }
}

/// Two honest clients plus whatever hostile update the test supplies.
fn batch_with(hostile: ClientDelta) -> Vec<ClientDelta> {
    vec![
        delta("honest-1", &[1.0, 1.0, 1.0], 10),
        delta("honest-2", &[1.1, 0.9, 1.0], 10),
        hostile,
    ]
}

/// The contract: never panic, and never return a non-finite value. An
/// `Err` is a pass — refusing a batch is a legitimate answer.
fn assert_survives(name: &str, updates: &[ClientDelta], scenario: &str) {
    let aggregator =
        build_aggregator(name, AggregatorParams::default()).expect("name is in the catalog");

    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        aggregator.aggregate(updates)
    }));

    match outcome {
        Err(_) => panic!("{name} PANICKED on {scenario} — a client can crash the server"),
        Ok(Err(_)) => { /* rejected: the correct response */ }
        Ok(Ok(output)) => {
            assert!(
                output.iter().all(|w| w.is_finite()),
                "{name} returned a non-finite aggregate on {scenario} ({output:?}) — this \
                 lands in the checkpoint and every later round starts from it"
            );
        }
    }
}

#[test]
fn no_aggregator_panics_or_emits_nan_when_a_client_submits_nan() {
    for &name in ALL_AGGREGATORS {
        assert_survives(
            name,
            &batch_with(delta("hostile", &[f32::NAN, f32::NAN, f32::NAN], 10)),
            "NaN weights",
        );
    }
}

#[test]
fn no_aggregator_panics_or_emits_nan_when_a_client_submits_infinity() {
    for &name in ALL_AGGREGATORS {
        assert_survives(
            name,
            &batch_with(delta("hostile", &[f32::INFINITY; 3], 10)),
            "positive infinity",
        );
        assert_survives(
            name,
            &batch_with(delta("hostile", &[f32::NEG_INFINITY; 3], 10)),
            "negative infinity",
        );
    }
}

#[test]
fn a_single_nan_coordinate_is_caught_not_just_an_all_nan_vector() {
    // The realistic shape: one exploded parameter in an otherwise
    // ordinary update, which is what a diverging real client produces.
    for &name in ALL_AGGREGATORS {
        assert_survives(
            name,
            &batch_with(delta("hostile", &[1.0, f32::NAN, 1.0], 10)),
            "one NaN coordinate among finite ones",
        );
    }
}

#[test]
fn nan_is_reported_with_the_client_and_the_coordinate() {
    // Not merely rejected — an operator needs to know who sent it and
    // which parameter blew up.
    let aggregator = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
    let err = aggregator
        .aggregate(&batch_with(delta("hostile", &[1.0, 1.0, f32::NAN], 10)))
        .expect_err("a NaN weight must be rejected");

    match err {
        AggregatorError::NonFiniteWeights { client_id, index } => {
            assert_eq!(client_id, "hostile");
            assert_eq!(index, 2);
        }
        other => panic!("expected NonFiniteWeights, got {other}"),
    }
}

#[test]
fn no_aggregator_is_captured_by_an_impossible_sample_count() {
    // Before validation: `fedavg` returned exactly [99.0, 99.0, 99.0] —
    // the liar's own submission, with every honest client numerically
    // erased.
    let hostile = delta("liar", &[99.0, 99.0, 99.0], u64::MAX);
    for &name in ALL_AGGREGATORS {
        let aggregator = build_aggregator(name, AggregatorParams::default()).unwrap();
        match aggregator.aggregate(&batch_with(hostile.clone())) {
            Err(_) => { /* rejected */ }
            Ok(output) => {
                assert!(
                    output.iter().all(|w| w.is_finite()),
                    "{name} produced a non-finite aggregate"
                );
                assert!(
                    output[0] < 50.0,
                    "{name} let one client with an impossible sample count capture the \
                     aggregate: got {output:?}, honest consensus is ~[1.0, 1.0, 1.0]"
                );
            }
        }
    }
}

#[test]
fn an_impossible_sample_count_names_the_client_and_the_limit() {
    let aggregator = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
    let err = aggregator
        .aggregate(&batch_with(delta("liar", &[99.0; 3], u64::MAX)))
        .expect_err("u64::MAX samples must be rejected");

    match err {
        AggregatorError::ImplausibleSampleCount {
            client_id,
            got,
            max,
        } => {
            assert_eq!(client_id, "liar");
            assert_eq!(got, u64::MAX);
            assert!(max < got);
        }
        other => panic!("expected ImplausibleSampleCount, got {other}"),
    }
}

#[test]
fn a_large_but_possible_sample_count_is_still_accepted() {
    // The check must not reject honest participants. A client with ten
    // million samples is unusual but real, and nothing may stop it.
    let aggregator = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
    let out = aggregator
        .aggregate(&batch_with(delta("big-but-real", &[2.0; 3], 10_000_000)))
        .expect("a plausible sample count must be accepted");
    assert!(out.iter().all(|w| w.is_finite()));
}

#[test]
fn zero_sample_counts_do_not_crash_any_aggregator() {
    // A client that trained on an empty shard. `fedavg` should weight it
    // at zero; nothing should divide by zero. An all-zero batch is the
    // degenerate case, and `ZeroWeightSum` is the right answer to it.
    for &name in ALL_AGGREGATORS {
        assert_survives(
            name,
            &batch_with(delta("empty-shard", &[1.0; 3], 0)),
            "num_samples = 0",
        );
        assert_survives(
            name,
            &[
                delta("a", &[1.0; 3], 0),
                delta("b", &[2.0; 3], 0),
                delta("c", &[3.0; 3], 0),
            ],
            "every client reporting zero samples",
        );
    }
}

#[test]
fn extreme_but_finite_magnitudes_do_not_produce_infinities() {
    // Just inside f32's range. The risk is intermediate overflow: a sum
    // of squares in a distance calculation reaches infinity long before
    // the inputs do.
    for &name in ALL_AGGREGATORS {
        assert_survives(
            name,
            &batch_with(delta("huge", &[f32::MAX, f32::MAX, f32::MAX], 10)),
            "f32::MAX weights",
        );
        assert_survives(
            name,
            &batch_with(delta("tiny", &[f32::MIN_POSITIVE; 3], 10)),
            "denormal-adjacent weights",
        );
    }
}

#[test]
fn degenerate_batch_shapes_do_not_panic() {
    for &name in ALL_AGGREGATORS {
        let aggregator = build_aggregator(name, AggregatorParams::default()).unwrap();

        // Empty batch: every method must say EmptyBatch, not panic.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| aggregator.aggregate(&[])));
        match outcome {
            Err(_) => panic!("{name} panicked on an empty batch"),
            Ok(Ok(_)) => panic!("{name} accepted an empty batch"),
            Ok(Err(AggregatorError::EmptyBatch)) => {}
            Ok(Err(other)) => panic!("{name} gave {other} for an empty batch, expected EmptyBatch"),
        }

        // A single client, and a one-dimensional model — both are edge
        // cases for methods that trim, split into groups, or compute
        // pairwise distances.
        assert_survives(name, &[delta("solo", &[1.0, 2.0], 10)], "a single client");
        assert_survives(
            name,
            &[
                delta("a", &[1.0], 10),
                delta("b", &[2.0], 10),
                delta("c", &[3.0], 10),
            ],
            "dim = 1",
        );
    }
}

#[test]
fn mismatched_dimensions_are_rejected_by_name() {
    let aggregator = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
    let err = aggregator
        .aggregate(&[delta("a", &[1.0, 2.0, 3.0], 10), delta("short", &[1.0], 10)])
        .expect_err("a dimension mismatch must be rejected");
    assert!(
        matches!(err, AggregatorError::MismatchedLength { .. }),
        "got {err}"
    );
}

#[test]
fn truncated_weight_bytes_are_rejected_not_silently_reinterpreted() {
    // Not a multiple of 4, so it cannot be a packed f32 vector. The risk
    // is a codec that rounds down and aggregates a prefix.
    let aggregator = build_aggregator("fedavg", AggregatorParams::default()).unwrap();
    let err = aggregator
        .aggregate(&[ClientDelta {
            client_id: "truncated".to_string(),
            round: 1,
            weights: vec![0u8; 7],
            num_samples: 10,
        }])
        .expect_err("7 bytes is not a valid f32 vector");
    assert!(
        matches!(err, AggregatorError::MalformedWeights { .. }),
        "got {err}"
    );
}
