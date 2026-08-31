//! The stateful aggregators, across rounds — the failure single-batch
//! tests structurally cannot reach.
//!
//! `tests/adversarial_input.rs` builds a fresh aggregator for every
//! assertion and hands it exactly one batch. That is the right shape for
//! the nine stateless methods, and it is blind for the four that are
//! not: `foolsgold`, `centered_clipping`, and `DssAggregator` all carry
//! state from one round into the next, behind a `Mutex` because
//! `Aggregator::aggregate` takes `&self` (ADR 0012).
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

use conflux_core::{
    Aggregator, AggregatorParams, CenteredClippingAggregator, DssAggregator, build_aggregator,
};
use conflux_proto::{ClientDelta, encode_weights};

/// The catalog methods that carry state across rounds. Spelled out
/// rather than derived, for the same reason `ALL_AGGREGATORS` is: a new
/// stateful method that is not added here is a visible omission.
const STATEFUL_CATALOG: &[&str] = &["foolsgold", "centered_clipping"];

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
/// a hostile one aggregates back near the honest consensus.
///
/// `centered_clipping` is deliberately excluded, and the exclusion is a
/// statement rather than a convenience. Its fidelity note (ADR 0008)
/// documents that `v` is seeded from round one's *plain mean*, so a
/// round-one attacker drags the reference and clipping then holds it
/// there — "the defense compounds over rounds rather than arriving
/// fully formed in round one". Asserting recovery here would assert
/// against a documented property of the published method. What it must
/// still guarantee is tested in
/// `centered_clipping_movement_stays_bounded_by_the_clip_radius`.
fn recovering_aggregators() -> Vec<(String, Box<dyn Aggregator>)> {
    stateful_aggregators()
        .into_iter()
        .filter(|(name, _)| name != "centered_clipping")
        .collect()
}

/// Every stateful aggregator under test, as `(name, instance)`.
///
/// `DssAggregator` is here and absent from `adversarial_input.rs` for
/// one reason: it is deliberately not in `build_aggregator`'s catalog
/// (it is an unvalidated hypothesis, see ADR 0008 and
/// `API_STABILITY.md`), so a suite that iterates catalog *names* cannot
/// see it. It is also the aggregator the DSS research line actually
/// drives, and the one whose stored state a research finding (§5.8.1,
/// "the shared deviation reference is itself unstable") already
/// questions — so leaving it untested was the least defensible gap of
/// the four.
fn stateful_aggregators() -> Vec<(String, Box<dyn Aggregator>)> {
    let mut all: Vec<(String, Box<dyn Aggregator>)> = STATEFUL_CATALOG
        .iter()
        .map(|&name| {
            (
                name.to_string(),
                build_aggregator(name, AggregatorParams::default())
                    .expect("name is in the catalog"),
            )
        })
        .collect();

    // Both DSS combine paths: `true` routes the final combine through
    // the wrapped base, `false` is DSS's own weighted mean. They are
    // different code, and only one of them was ever exercised here.
    for through_base in [true, false] {
        let mut dss = DssAggregator::new(
            build_aggregator("fedavg", AggregatorParams::default()).expect("fedavg is in catalog"),
        );
        dss.combine_through_base = through_base;
        all.push((
            format!("dss(combine_through_base={through_base})"),
            Box::new(dss),
        ));
    }

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
fn dss_survives_every_scenario_the_catalog_methods_are_held_to() {
    // T2: `adversarial_input.rs` iterates `build_aggregator`'s catalog,
    // and `DssAggregator` is deliberately not in it — so DSS had *no*
    // hostile-input coverage at all, single-batch or otherwise, despite
    // being the aggregator every DSS research run drives.
    //
    // This is the same scenario list the twelve catalog methods face,
    // applied to both DSS combine paths. Writing it found a real one:
    // `combine_through_base = false` returned `[inf, inf, inf]` on a
    // batch containing `f32::MAX`, because its weighted combine
    // multiplied each update by `weight × num_samples` *before*
    // normalizing, and `f32::MAX × 10` is already infinity.
    let scenarios: Vec<(&str, Vec<ClientDelta>)> = vec![
        (
            "NaN weights",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("hostile", &[f32::NAN, f32::NAN, f32::NAN], 10),
            ],
        ),
        (
            "one NaN coordinate",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("hostile", &[1.0, f32::NAN, 1.0], 10),
            ],
        ),
        (
            "positive infinity",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("hostile", &[f32::INFINITY; 3], 10),
            ],
        ),
        (
            "negative infinity",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("hostile", &[f32::NEG_INFINITY; 3], 10),
            ],
        ),
        (
            "u64::MAX sample count",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("liar", &[99.0, 99.0, 99.0], u64::MAX),
            ],
        ),
        (
            "every client reporting zero samples",
            vec![
                delta("a", &[1.0; 3], 0),
                delta("b", &[2.0; 3], 0),
                delta("c", &[3.0; 3], 0),
            ],
        ),
        (
            "f32::MAX weights",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("huge", &[f32::MAX; 3], 10),
            ],
        ),
        (
            "several clients at f32::MAX",
            vec![
                delta("a", &[f32::MAX; 3], 10),
                delta("b", &[f32::MAX; 3], 10),
                delta("c", &[f32::MAX; 3], 10),
            ],
        ),
        (
            "denormal-adjacent weights",
            vec![
                delta("honest", &[1.0, 1.0, 1.0], 10),
                delta("tiny", &[f32::MIN_POSITIVE; 3], 10),
            ],
        ),
        ("a single client", vec![delta("solo", &[1.0, 2.0], 10)]),
    ];

    for (name, aggregator) in stateful_aggregators() {
        if !name.starts_with("dss") {
            continue;
        }
        for (scenario, batch) in &scenarios {
            assert_round_survives(&name, &*aggregator, batch, scenario);
        }

        // Empty batch, which must be `EmptyBatch` rather than a panic.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| aggregator.aggregate(&[])));
        match outcome {
            Err(_) => panic!("{name} panicked on an empty batch"),
            Ok(Ok(_)) => panic!("{name} accepted an empty batch"),
            Ok(Err(_)) => {}
        }
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

#[test]
fn dss_diagnostics_stay_consistent_with_the_batch_they_describe() {
    // `last_diagnostics` is read by the research harness after every
    // round. If it can disagree with the batch that produced it — wrong
    // length, non-finite scores — every number the research line reports
    // is suspect, which matters more here than in most diagnostics.
    let mut dss = DssAggregator::new(
        build_aggregator("fedavg", AggregatorParams::default()).expect("fedavg is in catalog"),
    );
    dss.combine_through_base = true;

    for round in 0..10 {
        let batch = vec![
            delta("honest-1", &[1.0, 1.0, 1.0], 10),
            delta("honest-2", &[1.1, 0.9, 1.0], 10),
            delta("erratic", &[1e20 * (round as f32 + 1.0); 3], 10),
        ];

        if dss.aggregate(&batch).is_err() {
            continue;
        }

        let diagnostics = dss.last_diagnostics();
        assert_eq!(
            diagnostics.len(),
            batch.len(),
            "round {round}: {} diagnostics for {} clients",
            diagnostics.len(),
            batch.len()
        );
        for d in &diagnostics {
            assert!(
                d.stability.is_finite() && d.collusion.is_finite() && d.weight.is_finite(),
                "round {round}: non-finite diagnostic for {}: stability={}, collusion={}, \
                 weight={}",
                d.client_id,
                d.stability,
                d.collusion,
                d.weight
            );
        }
    }
}
