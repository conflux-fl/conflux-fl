//! The stateful aggregators, across rounds — the failure single-batch
//! tests structurally cannot reach.
//!
//! `tests/adversarial_input.rs` builds a fresh aggregator for every
//! assertion and hands it exactly one batch. That is the right shape for
//! the stateless methods, and it is blind for the ones that are not:
//! `foolsgold`, `centered_clipping`, `flanders`, and the whole
//! `optimization` family carry state from one round into the next,
//! behind a `Mutex` because `Aggregator::aggregate` takes `&self`.
//!
//! `decode_and_validate` guards the *inputs*. Nothing re-validates the
//! state derived from them. So there is a reachable sequence the
//! single-batch suite cannot express:
//!
//! 1. Round N carries weights that are hostile but **finite** — so
//!    validation accepts them, correctly.
//! 2. The aggregator folds them into its stored reference or history.
//! 3. Round N+1 is a perfectly ordinary batch from honest clients, and
//!    the poisoned state turns it into garbage.
//!
//! Nobody sent a bad update in round N+1. The output is wrong anyway,
//! and it is the checkpoint.
//!
//! The rule these tests encode extends the single-batch one: **an
//! aggregator's stored state must never turn a clean batch into a
//! non-finite or wildly wrong aggregate, whatever it was fed before.**
//! A batch may still be rejected — `Err` is a pass throughout.

use conflux_core::{Aggregator, AggregatorParams, CenteredClippingAggregator, build_aggregator};
use conflux_proto::{ClientDelta, encode_weights};

/// The catalog methods that carry state across rounds. Spelled out
/// rather than derived, because statefulness is not a registry fact: a
/// new stateful method that is not added here is a visible omission.
/// (`fltrust`/`zeno` hold only per-round injected state and refuse to
/// run without it, so they have nothing to carry between rounds.)
const STATEFUL_CATALOG: &[&str] = &[
    "foolsgold",
    "centered_clipping",
    "flanders",
    "fedavgm",
    "fedadagrad",
    "fedadam",
    "fedyogi",
    "qfedavg",
    "fednova",
    "scaffold",
];

/// The stateful methods whose published update rule bounds each round's
/// step relative to the *previous* model rather than re-centering on the
/// batch — so after an accepted extreme round they are not expected to
/// land back near the honest consensus in one clean round, and asserting
/// that would assert against the method:
///
/// - `centered_clipping`: its fidelity note documents that round one's
///   seed is a plain mean an attacker can drag, and clipping then holds
///   it — "the defense compounds over rounds rather than arriving fully
///   formed". What it must still guarantee is tested in
///   `centered_clipping_movement_stays_bounded_by_the_clip_radius`.
/// - `fedavgm`: momentum is the method; a huge delta lives in the buffer
///   for many rounds by design.
/// - `fedadagrad`/`fedadam`/`fedyogi`: the adaptive step is bounded by
///   roughly `η` per coordinate per round, so a model dragged to `1e38`
///   walks back one unit at a time — correct, and slow.
const BOUNDED_STEP: &[&str] = &[
    "centered_clipping",
    "fedavgm",
    "fedadagrad",
    "fedadam",
    "fedyogi",
];

fn delta(client_id: &str, weights: &[f32], num_samples: u64) -> ClientDelta {
    ClientDelta {
        client_id: client_id.to_string(),
        round: 1,
        weights: encode_weights(weights),
        num_samples,
        ..Default::default()
    }
}

/// An ordinary round: three clients agreeing closely on `[1, 1, 1]`.
/// Whatever came before, this must aggregate to something near it.
fn clean_batch() -> Vec<ClientDelta> {
    vec![
        delta("honest-1", &[1.0, 1.0, 1.0], 10),
        delta("honest-2", &[1.1, 0.9, 1.0], 10),
        delta("honest-3", &[0.9, 1.1, 1.0], 10),
    ]
}

/// The stateful aggregators expected to *recover* — a clean batch after
/// a hostile one aggregates back near the honest consensus. The
/// bounded-step methods are excluded, and the exclusion is a statement
/// rather than a convenience — see [`BOUNDED_STEP`].
fn recovering_aggregators() -> Vec<(String, Box<dyn Aggregator>)> {
    stateful_aggregators()
        .into_iter()
        .filter(|(name, _)| !BOUNDED_STEP.contains(&name.as_str()))
        .collect()
}

/// Every stateful aggregator under test, as `(name, instance)`.
///
/// Built by name from the catalog. An out-of-tree method that keeps
/// cross-round state is not visible to a suite that iterates catalog
/// names, and owes itself the equivalent coverage — see the module docs
/// above for the failure class this shape of test exists for.
fn stateful_aggregators() -> Vec<(String, Box<dyn Aggregator>)> {
    let all: Vec<(String, Box<dyn Aggregator>)> = STATEFUL_CATALOG
        .iter()
        .map(|&name| {
            (
                name.to_string(),
                build_aggregator(name, AggregatorParams::default())
                    .expect("name is in the catalog"),
            )
        })
        .collect();

    all
}

/// The contract, applied to one round's result: never panic, never
/// return a non-finite value. `Err` is a legitimate answer.
fn assert_round_survives(
    name: &str,
    aggregator: &dyn Aggregator,
    updates: &[ClientDelta],
    scenario: &str,
) -> Option<Vec<f32>> {
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        aggregator.aggregate(updates)
    }));

    match outcome {
        Err(_) => panic!("{name} PANICKED on {scenario} — a client can crash the server"),
        Ok(Err(_)) => None,
        Ok(Ok(output)) => {
            assert!(
                output.iter().all(|w| w.is_finite()),
                "{name} returned a non-finite aggregate on {scenario} ({output:?})"
            );
            Some(output)
        }
    }
}

#[test]
fn accepted_but_extreme_weights_do_not_poison_the_next_round() {
    // `f32::MAX` is finite, so `decode_and_validate` accepts it — as it
    // must, since the codec cannot know a model's plausible scale. The
    // question is what the aggregator *stores* afterwards.
    for (name, aggregator) in recovering_aggregators() {
        assert_round_survives(
            &name,
            &*aggregator,
            &[
                delta("honest-1", &[1.0, 1.0, 1.0], 10),
                delta("honest-2", &[1.1, 0.9, 1.0], 10),
                delta("extreme", &[f32::MAX, f32::MAX, f32::MAX], 10),
            ],
            "round 1: an accepted f32::MAX update",
        );

        // Round 2 is clean. Nobody is attacking any more.
        let out = assert_round_survives(
            &name,
            &*aggregator,
            &clean_batch(),
            "round 2: a clean batch after an f32::MAX round",
        );

        if let Some(out) = out {
            assert!(
                out.iter().all(|w| w.abs() < 1e6),
                "{name} let round 1's extreme update wreck a clean round 2: got {out:?}, \
                 every client submitted ~[1.0, 1.0, 1.0]"
            );
        }
    }
}

#[test]
fn a_rejected_batch_does_not_corrupt_state_for_later_rounds() {
    // The other order: establish healthy state, feed a batch that gets
    // rejected, then a clean one. A rejection that has already mutated
    // half the state before returning `Err` would show up here.
    for (name, aggregator) in stateful_aggregators() {
        assert_round_survives(&name, &*aggregator, &clean_batch(), "round 1: clean");

        let mut poisoned = clean_batch();
        poisoned.push(delta("hostile", &[f32::NAN, f32::NAN, f32::NAN], 10));
        let rejected = aggregator.aggregate(&poisoned);
        assert!(
            rejected.is_err(),
            "{name} accepted a NaN batch — adversarial_input.rs should have caught this"
        );

        let out = assert_round_survives(
            &name,
            &*aggregator,
            &clean_batch(),
            "round 3: clean, after a rejected round 2",
        );

        if let Some(out) = out {
            assert!(
                out.iter().all(|w| w.abs() < 1e6),
                "{name} carried damage out of a *rejected* batch into round 3: got {out:?}"
            );
        }
    }
}

#[test]
fn a_long_run_of_extreme_rounds_does_not_diverge() {
    // Single extremes can be absorbed; the question is whether repeated
    // ones accumulate. A running reference updated by `ref += delta`
    // every round drifts without bound even when each round is
    // individually reasonable.
    for (name, aggregator) in recovering_aggregators() {
        for round in 0..25 {
            // Alternating sign, so a method that tracks a running mean
            // cannot simply settle: each round pulls it the other way.
            let magnitude = if round % 2 == 0 { 1e30 } else { -1e30 };
            assert_round_survives(
                &name,
                &*aggregator,
                &[
                    delta("honest-1", &[1.0, 1.0, 1.0], 10),
                    delta("honest-2", &[1.1, 0.9, 1.0], 10),
                    delta("oscillator", &[magnitude, magnitude, magnitude], 10),
                ],
                &format!("round {round} of an alternating ±1e30 run"),
            );
        }

        let out = assert_round_survives(
            &name,
            &*aggregator,
            &clean_batch(),
            "a clean batch after 25 extreme rounds",
        );

        if let Some(out) = out {
            assert!(
                out.iter().all(|w| w.abs() < 1e6),
                "{name} diverged over 25 extreme rounds and never recovered: got {out:?}"
            );
        }
    }
}

#[test]
fn a_client_that_changes_dimension_between_rounds_does_not_panic() {
    // Stored per-client history is keyed by client id, not by shape. A
    // client that submits 3 weights in round 1 and 5 in round 2 is
    // either misconfigured or probing; either way, comparing this
    // round's vector against a stored one of a different length is an
    // index-out-of-bounds waiting to happen.
    for (name, aggregator) in stateful_aggregators() {
        assert_round_survives(&name, &*aggregator, &clean_batch(), "round 1: 3 weights");

        assert_round_survives(
            &name,
            &*aggregator,
            &[
                delta("honest-1", &[1.0, 1.0, 1.0, 1.0, 1.0], 10),
                delta("honest-2", &[1.1, 0.9, 1.0, 1.0, 1.0], 10),
                delta("honest-3", &[0.9, 1.1, 1.0, 1.0, 1.0], 10),
            ],
            "round 2: the same clients, now 5 weights",
        );

        assert_round_survives(
            &name,
            &*aggregator,
            &clean_batch(),
            "round 3: back to 3 weights",
        );
    }
}

#[test]
fn a_completely_new_client_set_each_round_does_not_grow_state_without_bound() {
    // Cross-device topologies sample a different cohort every round, so
    // per-client history keyed by id grows once per client *ever seen*,
    // not once per participant. 200 rounds of fresh ids is an ordinary
    // week for such a deployment, and a hostile client can make its id
    // fresh on purpose.
    for (name, aggregator) in stateful_aggregators() {
        for round in 0..200 {
            assert_round_survives(
                &name,
                &*aggregator,
                &[
                    delta(&format!("r{round}-a"), &[1.0, 1.0, 1.0], 10),
                    delta(&format!("r{round}-b"), &[1.1, 0.9, 1.0], 10),
                    delta(&format!("r{round}-c"), &[0.9, 1.1, 1.0], 10),
                ],
                &format!("round {round} with an entirely new cohort"),
            );
        }

        let out = assert_round_survives(
            &name,
            &*aggregator,
            &clean_batch(),
            "a clean batch after 200 all-new cohorts",
        );

        if let Some(out) = out {
            assert!(
                out.iter().all(|w| w.abs() < 1e6),
                "{name} misbehaved after 200 rounds of unseen clients: got {out:?}"
            );
        }
    }
}

#[test]
fn centered_clippings_stored_reference_stays_finite() {
    // `CenteredClippingAggregator` exposes its reference, so this can
    // assert on the state itself rather than inferring it from output —
    // the stored vector is what round N+1 clips against, and a
    // non-finite one makes every later round meaningless.
    let aggregator = CenteredClippingAggregator::new(1.0);

    for round in 0..25 {
        let magnitude = if round % 2 == 0 { f32::MAX } else { f32::MIN };
        let _ = aggregator.aggregate(&[
            delta("honest", &[1.0, 1.0, 1.0], 10),
            delta("extreme", &[magnitude, magnitude, magnitude], 10),
        ]);

        let reference = aggregator
            .reference()
            .expect("a reference exists after the first round");
        assert!(
            reference.iter().all(|w| w.is_finite()),
            "the stored reference went non-finite at round {round}: {reference:?} — every \
             later round clips against this"
        );
    }
}

#[test]
fn centered_clipping_movement_stays_bounded_by_the_clip_radius() {
    // What Centered Clipping actually promises, and the reason
    // `recovering_aggregators` excludes it from the recovery tests.
    //
    // Its fidelity note is explicit that round one's seed is a plain
    // mean an attacker can drag, so "does it come back to ~1.0?" is the
    // wrong question — the method never claimed it would. The claim it
    // *does* make is the one worth guarding: after the seed, no single
    // round may move the reference by more than `τ`-worth of pull, no
    // matter what any client submits. That is the entire defense. A
    // round that moves it further means clipping did not bind, and an
    // attacker has unbounded influence again.
    let tau = 1.0f32;
    let aggregator = CenteredClippingAggregator::new(tau);

    // Seed on an ordinary batch so the reference starts somewhere sane.
    let seed = aggregator
        .aggregate(&clean_batch())
        .expect("a clean batch seeds the reference");

    let mut previous = seed;
    for round in 0..30 {
        // Alternating extremes, each individually finite and accepted.
        let magnitude = if round % 2 == 0 { 1e30 } else { -1e30 };
        let next = aggregator
            .aggregate(&[
                delta("honest-1", &[1.0, 1.0, 1.0], 10),
                delta("honest-2", &[1.1, 0.9, 1.0], 10),
                delta("attacker", &[magnitude, magnitude, magnitude], 10),
            ])
            .expect("finite weights are accepted");

        assert!(
            next.iter().all(|w| w.is_finite()),
            "round {round}: reference went non-finite: {next:?}"
        );

        // ‖v_next − v_prev‖ ≤ τ. Each client contributes at most τ of
        // pull and the combine averages them, so the mean displacement
        // cannot exceed τ however extreme one client is. A small
        // tolerance covers f32 rounding at these magnitudes.
        let moved = next
            .iter()
            .zip(&previous)
            .map(|(a, b)| {
                let d = *a as f64 - *b as f64;
                d * d
            })
            .sum::<f64>()
            .sqrt();

        assert!(
            moved <= tau as f64 * 1.01,
            "round {round}: the reference moved {moved} in one round, but τ = {tau} is \
             supposed to bound exactly this — an attacker regained unbounded pull"
        );

        previous = next;
    }
}
