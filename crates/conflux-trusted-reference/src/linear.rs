//! A real, working [`TrustedModel`]: linear least squares by gradient
//! descent.
//!
//! Shipped so the sidecar boundary has a genuine implementation behind it
//! rather than a stub — this trains, and a test asserts it recovers known
//! coefficients from a wrong starting point. It is also the honest limit
//! of what belongs in a crate with no ML runtime: it is a *linear* model,
//! so it is a faithful trusted reference for a linear task and nothing
//! else.
//!
//! FLTrust's own paper uses a root dataset of around a hundred examples,
//! which is the scale this is built for. A deployment training a
//! convolutional network implements [`TrustedModel`] against a real
//! runtime instead; that is the extension point, and this is the worked
//! example of using it.

use crate::TrustedModel;

/// Ordinary least squares, `ŷ = w · x`, trained by full-batch gradient
/// descent on a fixed trusted dataset.
///
/// No bias term: Conflux transmits a flat weight vector whose meaning is
/// the client's business (ADR 0004), so a caller that wants a bias adds a
/// constant `1.0` feature itself. Inventing an extra parameter here would
/// make the sidecar's vector a different length than the model's, which
/// is the one thing the length check downstream exists to catch.
pub struct LinearLeastSquares {
    /// `(features, target)` pairs. The trusted root dataset — the single
    /// piece of data in the whole system that the defense's integrity
    /// rests on, which is why ADR 0011 puts it behind a boundary the
    /// server itself cannot be tricked into crossing.
    dataset: Vec<(Vec<f32>, f32)>,
    learning_rate: f32,
    steps: usize,
}

impl LinearLeastSquares {
    /// A model over `dataset`, trained for `steps` full-batch gradient
    /// steps at `learning_rate`.
    ///
    /// Both knobs are explicit rather than defaulted: FLTrust assumes the
    /// server's own training effort is comparable to a client's, and only
    /// the deployer knows what its clients do. A reference trained far
    /// less than the clients were is still a valid vector — it simply
    /// points less far in the right direction, which quietly weakens
    /// every trust score computed against it.
    pub fn new(dataset: Vec<(Vec<f32>, f32)>, learning_rate: f32, steps: usize) -> Self {
        Self {
            dataset,
            learning_rate,
            steps,
        }
    }

    /// Mean squared error of `weights` over the dataset.
    fn loss(&self, weights: &[f32]) -> f64 {
        if self.dataset.is_empty() {
            return 0.0;
        }
        // `f64` throughout, for the same reason `conflux-core`'s
        // distances are: a squared residual on a diverged candidate
        // overflows `f32` long before the candidate itself is
        // unreasonable, and an infinite loss compared against another
        // infinite loss yields `NaN` — which every comparison then
        // treats as "not worse", exactly backwards for a scoring
        // function.
        let mut total = 0.0f64;
        for (features, target) in &self.dataset {
            let prediction: f64 = features
                .iter()
                .zip(weights)
                .map(|(x, w)| *x as f64 * *w as f64)
                .sum();
            let residual = prediction - *target as f64;
            total += residual * residual;
        }
        total / self.dataset.len() as f64
    }
}

impl TrustedModel for LinearLeastSquares {
    fn train_reference(&self, global_weights: &[f32]) -> Vec<f32> {
        // A dataset whose feature width disagrees with the model being
        // trained cannot produce a usable reference. Returning the input
        // unchanged is the documented contract: it is the right length,
        // so it is rejected by an aggregator's own checks as "no
        // improvement" rather than being mistaken for a real reference of
        // some other shape.
        let dim = global_weights.len();
        if self.dataset.is_empty() || self.dataset.iter().any(|(f, _)| f.len() != dim) {
            return global_weights.to_vec();
        }

        let mut weights: Vec<f64> = global_weights.iter().map(|w| *w as f64).collect();
        let n = self.dataset.len() as f64;
        let lr = self.learning_rate as f64;

        for _ in 0..self.steps {
            // d/dw of (1/n) Σ (w·x − y)² is (2/n) Σ (w·x − y) x.
            let mut gradient = vec![0.0f64; dim];
            for (features, target) in &self.dataset {
                let prediction: f64 = features
                    .iter()
                    .zip(&weights)
                    .map(|(x, w)| *x as f64 * w)
                    .sum();
                let residual = prediction - *target as f64;
                for (g, x) in gradient.iter_mut().zip(features) {
                    *g += 2.0 * residual * *x as f64 / n;
                }
            }
            for (w, g) in weights.iter_mut().zip(&gradient) {
                *w -= lr * g;
            }

            // A learning rate too large for this dataset diverges, and a
            // diverged reference is worse than none: it is a finite-
            // looking vector pointing nowhere, and FLTrust would score
            // every honest client against it. Stopping at the last
            // finite iterate turns a silent wrong answer into a visibly
            // useless one.
            if weights.iter().any(|w| !w.is_finite()) {
                return global_weights.to_vec();
            }
        }

        weights.into_iter().map(|w| w as f32).collect()
    }

    fn score(&self, global_weights: &[f32], candidate: &[f32]) -> f32 {
        if candidate.len() != global_weights.len() {
            // Not scoreable. `f32::NEG_INFINITY` would be a strong
            // opinion; this is an absence of one, and the service omits
            // unscoreable candidates rather than reporting a number.
            return f32::NAN;
        }
        let improvement = self.loss(global_weights) - self.loss(candidate);
        // Clamped into `f32` range rather than allowed to become
        // infinity: a candidate that is merely very bad and one that
        // overflowed should not become indistinguishable.
        improvement.clamp(f32::MIN as f64, f32::MAX as f64) as f32
    }

    fn model_dim(&self) -> Option<u64> {
        self.dataset.first().map(|(f, _)| f.len() as u64)
    }

    fn description(&self) -> String {
        format!(
            "linear least squares, {} trusted examples, {} steps @ lr {}",
            self.dataset.len(),
            self.steps,
            self.learning_rate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `y = 2x₀ + 3x₁`, exactly recoverable.
    fn dataset() -> Vec<(Vec<f32>, f32)> {
        vec![
            (vec![1.0, 0.0], 2.0),
            (vec![0.0, 1.0], 3.0),
            (vec![1.0, 1.0], 5.0),
            (vec![2.0, 1.0], 7.0),
            (vec![1.0, 2.0], 8.0),
        ]
    }

    #[test]
    fn it_actually_trains_rather_than_returning_its_input() {
        // The claim the whole sidecar rests on: this is real training,
        // not a stub with a plausible signature.
        let model = LinearLeastSquares::new(dataset(), 0.05, 2000);
        let reference = model.train_reference(&[0.0, 0.0]);

        assert!(
            (reference[0] - 2.0).abs() < 0.05 && (reference[1] - 3.0).abs() < 0.05,
            "should recover [2, 3] from a zero start, got {reference:?}"
        );
    }

    #[test]
    fn it_converges_from_a_deliberately_wrong_start() {
        // Starting far from the solution, and in the wrong direction.
        let model = LinearLeastSquares::new(dataset(), 0.05, 2000);
        let reference = model.train_reference(&[-40.0, 25.0]);

        assert!(
            (reference[0] - 2.0).abs() < 0.05 && (reference[1] - 3.0).abs() < 0.05,
            "should still reach [2, 3], got {reference:?}"
        );
    }

    #[test]
    fn scoring_prefers_the_better_candidate() {
        let model = LinearLeastSquares::new(dataset(), 0.05, 100);
        let global = [0.0, 0.0];

        let correct = model.score(&global, &[2.0, 3.0]);
        let close = model.score(&global, &[1.8, 3.2]);
        let wrong = model.score(&global, &[-6.0, 11.0]);

        assert!(correct > close, "{correct} vs {close}");
        assert!(close > wrong, "{close} vs {wrong}");
        assert!(wrong < 0.0, "a worse-than-global candidate scores negative");
    }

    #[test]
    fn a_diverging_learning_rate_returns_the_input_rather_than_garbage() {
        // A reference that overflowed is worse than no reference: it is a
        // finite-looking vector pointing nowhere, and every honest client
        // would be scored against it.
        let model = LinearLeastSquares::new(dataset(), 1e6, 500);
        let reference = model.train_reference(&[1.0, 1.0]);

        assert!(reference.iter().all(|w| w.is_finite()), "got {reference:?}");
        assert_eq!(reference, vec![1.0, 1.0], "falls back to the input");
    }

    #[test]
    fn a_dimension_mismatch_is_refused_not_guessed() {
        let model = LinearLeastSquares::new(dataset(), 0.05, 100);
        // The dataset has 2 features; the model being trained has 5
        // weights. There is no correct answer, so it must not invent one.
        let reference = model.train_reference(&[0.0; 5]);
        assert_eq!(reference, vec![0.0; 5]);

        assert!(model.score(&[0.0; 5], &[0.0; 3]).is_nan());
    }

    #[test]
    fn an_extreme_candidate_scores_finitely_rather_than_overflowing() {
        // `f32::MAX` squared overflows `f32`. If the loss were computed
        // there, this would be `-inf` — or `NaN` once compared against
        // another overflowed score — and a scoring function that cannot
        // rank its worst inputs is not one.
        let model = LinearLeastSquares::new(dataset(), 0.05, 100);
        let score = model.score(&[0.0, 0.0], &[f32::MAX, f32::MAX]);

        assert!(score.is_finite(), "got {score}");
        assert!(score < 0.0, "an absurd candidate must score below global");
    }

    #[test]
    fn an_empty_dataset_yields_no_opinion_rather_than_a_wrong_one() {
        let model = LinearLeastSquares::new(Vec::new(), 0.05, 100);
        assert_eq!(model.train_reference(&[1.0, 2.0]), vec![1.0, 2.0]);
        assert_eq!(model.score(&[1.0, 2.0], &[9.0, 9.0]), 0.0);
    }
}
