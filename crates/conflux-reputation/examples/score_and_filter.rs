//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-reputation`](https://confluxfl.dev/crate-deep-dives/conflux-reputation/):
//! scores a handful of clients' updates against a reference direction,
//! then filters them by a similarity threshold -- the two operations
//! `conflux-reputation` provides. Run with:
//!
//! ```bash
//! cargo run --example score_and_filter -p conflux-reputation
//! ```

use conflux_reputation::{ContributionScorer, CosineScorer, filter_by_threshold};

fn main() {
    let scorer = CosineScorer;

    // A reference direction -- in a real round this would typically be the
    // mean of all submitted updates. Here it just points along the first
    // coordinate.
    let reference = vec![1.0, 0.0, 0.0];

    let updates = vec![
        ("client-aligned".to_string(), vec![2.0, 0.0, 0.0]),
        ("client-orthogonal".to_string(), vec![0.0, 1.0, 0.0]),
        ("client-opposite".to_string(), vec![-1.0, 0.0, 0.0]),
        ("client-slightly-off".to_string(), vec![1.0, 0.2, 0.0]),
    ];

    println!("scoring each client's update against the reference direction {reference:?}:");
    for (id, update) in &updates {
        let score = scorer.score(update, &reference);
        println!("  {id}: update = {update:?}, score = {score:.4}");
    }

    let min_score = 0.5;
    let passed = filter_by_threshold(&updates, &reference, &scorer, min_score);
    println!("\nfilter_by_threshold(min_score = {min_score}) keeps: {passed:?}");

    // A submission with a non-finite value scores NaN, and NaN never
    // satisfies `>=` against any threshold -- including the most
    // permissive one, -1.0 (cosine similarity's theoretical floor).
    let mut updates_with_broken_client = updates.clone();
    updates_with_broken_client.push(("client-broken".to_string(), vec![f32::NAN, 0.0, 0.0]));
    let broken_score = scorer.score(&[f32::NAN, 0.0, 0.0], &reference);
    let passed_permissive =
        filter_by_threshold(&updates_with_broken_client, &reference, &scorer, -1.0);
    println!(
        "\nclient-broken's update contains a NaN: score = {broken_score} (is_nan = {})",
        broken_score.is_nan()
    );
    println!(
        "filter_by_threshold(min_score = -1.0) still keeps only: {passed_permissive:?} \
         (client-broken excluded even at the most permissive possible threshold)"
    );

    // filter_by_threshold trusts `reference` -- it doesn't validate it. If
    // the reference itself is built from an unvalidated NaN submission
    // (e.g. an unfiltered batch mean), every client's score against that
    // reference becomes NaN too, and all of them are rejected together --
    // not just the client that caused it.
    let poisoned_reference = vec![f32::NAN, 0.0, 0.0];
    let honest_updates = vec![("client-honest".to_string(), vec![1.0, 0.0, 0.0])];
    let passed_against_poisoned =
        filter_by_threshold(&honest_updates, &poisoned_reference, &scorer, -1.0);
    println!(
        "\nwith a poisoned (NaN) reference vector, even client-honest's clean update is rejected: {passed_against_poisoned:?}"
    );
}
