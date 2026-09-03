//! Contribution scoring for federated learning updates: measures how well
//! each client's submitted update agrees with a reference direction, and
//! filters out submissions that fall below a configured similarity
//! threshold before aggregation.
//!
//! This crate is entirely opt-in and has no side effects of its own — a
//! caller decides whether to build a reference vector and call
//! [`filter_by_threshold`] at all, or to hand every submitted update
//! straight to the aggregator unfiltered. When it is used, it sits between
//! a privacy transform (if any) and aggregation: a caller scores and
//! filters the already-decoded, already-privatized updates, then passes
//! only the surviving client ids' updates on to the aggregator.
//!
//! # Example
//!
//! ```
//! use conflux_reputation::{CosineScorer, filter_by_threshold};
//!
//! // The direction the server expects this round to move in.
//! let reference = vec![1.0_f32, 1.0, 1.0];
//!
//! let updates = vec![
//!     ("aligned".to_string(), vec![0.9_f32, 1.1, 1.0]),
//!     ("orthogonal".to_string(), vec![1.0_f32, -1.0, 0.0]),
//!     ("inverted".to_string(), vec![-1.0_f32, -1.0, -1.0]),
//! ];
//!
//! let kept = filter_by_threshold(&updates, &reference, &CosineScorer, 0.5);
//! assert_eq!(kept, vec!["aligned".to_string()]);
//!
//! // Cosine similarity, so the score is direction-only: magnitude does
//! // not buy influence, and a client pointing the opposite way scores
//! // negative rather than merely small.
//! use conflux_reputation::ContributionScorer;
//! assert!((CosineScorer.score(&[2.0, 2.0, 2.0], &reference) - 1.0).abs() < 1e-6);
//! assert!(CosineScorer.score(&[-1.0, -1.0, -1.0], &reference) < 0.0);
//! ```

#![warn(missing_docs)]

/// Scores one client's update against a reference direction (typically the
/// mean, or some other consensus signal, computed across the round's
/// submissions). Higher means more similar to that reference; the exact
/// scale and sign depend on the implementation. Scoring alone rejects
/// nothing — it's the input to threshold-based filtering, below.
pub trait ContributionScorer: Send + Sync {
    /// Scores `update` against `reference`. Both slices are raw weight
    /// vectors of the same length.
    ///
    /// Returns a bare `f32` rather than a `Result`: a scorer compares two
    /// vectors it was handed and always has an answer, so there is no
    /// failure to report. Whether that answer is *good enough* is the
    /// caller's threshold decision, not this method's.
    fn score(&self, update: &[f32], reference: &[f32]) -> f32;
}

/// Cosine similarity between an update and the reference direction: `1.0`
/// for identical direction, `0.0` for orthogonal, `-1.0` for opposite —
/// the full range for any two non-zero vectors whose entries are all
/// finite. An update pointing in a very different direction from the
/// reference scores low, which is the signal a caller can use to catch
/// outlier or adversarial submissions. Returns `0.0` rather than
/// dividing by zero if either vector is the zero vector, since cosine
/// similarity is undefined there.
///
/// Two vectors of different lengths are not comparable at all, and score
/// `NaN` — the one value [`filter_by_threshold`] can never accept. A
/// silent prefix comparison would let a truncated update score as if it
/// were whole.
pub struct CosineScorer;

impl ContributionScorer for CosineScorer {
    fn score(&self, update: &[f32], reference: &[f32]) -> f32 {
        if update.len() != reference.len() {
            return f32::NAN;
        }
        let dot: f32 = update.iter().zip(reference).map(|(a, b)| a * b).sum();
        let norm_update = l2_norm(update);
        let norm_reference = l2_norm(reference);
        if norm_update == 0.0 || norm_reference == 0.0 {
            return 0.0;
        }
        dot / (norm_update * norm_reference)
    }
}

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

/// Filters `updates` against `reference` using `scorer`, keeping only the
/// ids whose score is `>= min_score`. Every rejected update is logged
/// (client id, score, and the threshold it missed) via `tracing::warn!`,
/// so a rejection is always visible in the server's logs rather than
/// silently dropped.
///
/// This function trusts `reference` and every entry in `updates` to
/// already be well-formed — it only compares scores against a threshold,
/// it doesn't validate its inputs. That matters for `NaN`: if `reference`
/// or an update contains one, the resulting score is `NaN`, and `NaN >=
/// min_score` is `false` for every `min_score`, including `-1.0` (cosine
/// similarity's theoretical floor) — so a single `NaN` score can never
/// pass this filter, no matter how permissive the threshold. That's a
/// safe outcome for one corrupted score, but it does not protect a
/// `reference` vector that is already `NaN` in some coordinate before it
/// gets here: every update scored against a `NaN` reference comes back
/// `NaN` too, which rejects every client that round, not just whichever
/// one produced the bad value originally. Keeping `reference` itself
/// built from already-finite inputs is what avoids that failure mode.
pub fn filter_by_threshold(
    updates: &[(String, Vec<f32>)],
    reference: &[f32],
    scorer: &dyn ContributionScorer,
    min_score: f32,
) -> Vec<String> {
    updates
        .iter()
        .filter_map(|(id, weights)| {
            let score = scorer.score(weights, reference);
            if score >= min_score {
                Some(id.clone())
            } else {
                tracing::warn!(
                    client_id = %id,
                    score,
                    threshold = min_score,
                    "update rejected by reputation filter"
                );
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_vectors_score_one() {
        let scorer = CosineScorer;

        assert!((scorer.score(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn opposite_vectors_score_negative_one() {
        let scorer = CosineScorer;

        assert!((scorer.score(&[1.0, 2.0, 3.0], &[-1.0, -2.0, -3.0]) - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn orthogonal_vectors_score_zero() {
        let scorer = CosineScorer;

        assert!(scorer.score(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    #[tracing_test::traced_test]
    fn filter_logs_rejections_with_client_and_scores() {
        let reference = vec![1.0, 0.0];
        let updates = vec![
            ("aligned".to_string(), vec![1.0, 0.0]),
            ("opposite".to_string(), vec![-1.0, 0.0]),
        ];

        filter_by_threshold(&updates, &reference, &CosineScorer, 0.5);

        assert!(logs_contain("update rejected by reputation filter"));
        assert!(logs_contain("client_id=opposite"));
        assert!(logs_contain("threshold=0.5"));
        // The accepted update must not appear in a rejection log.
        assert!(!logs_contain("client_id=aligned"));
    }

    #[test]
    fn filter_excludes_updates_below_threshold() {
        let reference = vec![1.0, 0.0];
        let updates = vec![
            ("aligned".to_string(), vec![1.0, 0.0]),
            ("orthogonal".to_string(), vec![0.0, 1.0]),
            ("opposite".to_string(), vec![-1.0, 0.0]),
        ];

        let passed = filter_by_threshold(&updates, &reference, &CosineScorer, 0.5);

        assert_eq!(passed, vec!["aligned".to_string()]);
    }

    #[test]
    fn filter_includes_updates_at_or_above_threshold() {
        let reference = vec![1.0, 0.0];
        let updates = vec![("aligned".to_string(), vec![1.0, 0.0])];

        let passed = filter_by_threshold(&updates, &reference, &CosineScorer, 1.0);

        assert_eq!(passed, vec!["aligned".to_string()]);
    }

    #[test]
    fn filter_with_empty_updates_returns_empty_without_panicking() {
        let reference = vec![1.0, 0.0];
        let updates: Vec<(String, Vec<f32>)> = Vec::new();

        let passed = filter_by_threshold(&updates, &reference, &CosineScorer, -1.0);

        assert!(passed.is_empty());
    }

    #[test]
    fn a_nan_update_scores_nan_and_is_rejected_even_at_the_lowest_threshold() {
        // -1.0 is cosine similarity's theoretical floor -- the most
        // permissive threshold a caller could configure. A NaN score must
        // still fail `score >= min_score` here, since a comparison against
        // NaN is always false, regardless of which operand is NaN or what
        // the threshold is.
        let reference = vec![1.0, 0.0];
        let updates = vec![("broken".to_string(), vec![f32::NAN, 0.0])];

        let score = CosineScorer.score(&updates[0].1, &reference);
        assert!(score.is_nan());

        let passed = filter_by_threshold(&updates, &reference, &CosineScorer, -1.0);
        assert!(passed.is_empty());
    }

    #[test]
    fn a_length_mismatch_scores_nan_rather_than_a_prefix() {
        // A truncated update must not score like a whole one: NaN is the
        // one value the filter can never accept.
        let score = CosineScorer.score(&[1.0, 0.0], &[1.0, 0.0, 0.0]);
        assert!(score.is_nan());

        let updates = vec![("short".to_string(), vec![1.0, 0.0])];
        let passed = filter_by_threshold(&updates, &[1.0, 0.0, 0.0], &CosineScorer, -1.0);
        assert!(passed.is_empty());
    }

    #[test]
    fn a_nan_reference_poisons_every_update_scored_against_it() {
        // `filter_by_threshold` trusts its `reference` argument -- it
        // doesn't validate it. If a caller builds `reference` from
        // unvalidated inputs (e.g. an unfiltered batch mean) and one
        // client's submission was non-finite, every other client's
        // otherwise-honest update now scores NaN too, and all of them are
        // rejected together. This documents that real failure mode rather
        // than asserting it away: the fix belongs upstream, in how the
        // caller builds `reference` in the first place, not in this
        // function.
        let poisoned_reference = vec![f32::NAN, 0.0];
        let updates = vec![("honest".to_string(), vec![1.0, 0.0])];

        let passed = filter_by_threshold(&updates, &poisoned_reference, &CosineScorer, -1.0);

        assert!(passed.is_empty());
    }
}
