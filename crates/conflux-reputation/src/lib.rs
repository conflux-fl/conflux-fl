//! Contribution scoring, Byzantine detection.
//!
//! See `docs/spec/conflux-spec-v1.md` §8.

/// Scores one client's update against a reference direction — the input to
/// Byzantine-resilience filtering. Spec §8: this sits between
/// `conflux-privacy`'s server-side transform and `conflux-core`'s
/// aggregation.
pub trait ContributionScorer: Send + Sync {
    fn score(&self, update: &[f32], reference: &[f32]) -> f32;
}

/// Cosine similarity between an update and the round's reference direction
/// (e.g. the mean of all updates) — plan §10's Phase 2 member. An update
/// pointing in a very different direction from consensus scores low;
/// that's the Byzantine/outlier signal.
pub struct CosineScorer;

impl ContributionScorer for CosineScorer {
    fn score(&self, update: &[f32], reference: &[f32]) -> f32 {
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

/// Filters `updates` against `reference` using `scorer`, keeping only ids
/// whose score is `>= min_score`. Logs every rejection — ADR 0007:
/// "`conflux-reputation` logs every rejected update with its score and
/// threshold."
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
}
