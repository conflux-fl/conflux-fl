//! The `robust` (Byzantine-resilient) aggregation family (spec §5).
//!
//! Two composable shapes rather than one (Phase 11a). The first pairs
//! `UpdateFilter` with `FilteredAggregator<F, C>`, for methods that pick
//! a *subset of whole updates* to keep (Krum, Multi-Krum). The second
//! pairs `CoordinateWiseRobustStatistic` with `CoordinateWiseAggregator<S>`,
//! for methods that combine *one coordinate at a time across every
//! client* (Trimmed Mean, Median) — these don't fit "selected whole
//! updates" at all, so forcing them through the first shape would
//! misrepresent what they compute.
//!
//! See `docs/phases/phase-11a-robust-aggregation.md` for the full
//! rationale, including why this split (rather than one shape, or two
//! unrelated ones) is what lets a future method needing *both* — e.g.
//! Bulyan, El Mhamdi, Guerraoui & Rouault (2018), *The Hidden
//! Vulnerability of Distributed Learning in Byzantine Settings*, ICML —
//! compose as `FilteredAggregator<SomeFilter,
//! CoordinateWiseAggregator<SomeStatistic>>` without changing anything in
//! this module.

use conflux_proto::ClientDelta;

use crate::weights::decode_and_validate;
use crate::{Aggregator, AggregatorError};

/// Pairwise L2 distances between a batch's decoded weight vectors — the
/// shared input Krum/Multi-Krum reason about (each update's score is a
/// function of its nearest neighbors). Trimmed Mean/Median never build
/// one — it's Krum/Multi-Krum-specific, not "robust family"-wide.
pub struct DistanceMatrix {
    distances: Vec<Vec<f32>>,
}

impl DistanceMatrix {
    pub fn from_updates(updates: &[ClientDelta]) -> Result<Self, AggregatorError> {
        let decoded = decode_and_validate(updates)?;

        let n = decoded.len();
        let mut distances = vec![vec![0.0f32; n]; n];
        for i in 0..n {
            for j in (i + 1)..n {
                let d = l2_distance(&decoded[i], &decoded[j]);
                distances[i][j] = d;
                distances[j][i] = d;
            }
        }

        Ok(Self { distances })
    }

    pub fn distance(&self, i: usize, j: usize) -> f32 {
        self.distances[i][j]
    }

    pub fn len(&self) -> usize {
        self.distances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.distances.is_empty()
    }
}

fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Which updates in a batch a filter judges trustworthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionResult {
    pub selected_indices: Vec<usize>,
}

/// What varies about which updates a selection-based `robust` member
/// trusts, given a batch. Deliberately distinct from
/// `conflux-selector::ClientSelector` — that trait answers "who trains
/// this round," decided *before* any update exists; this one answers
/// "which of the updates that came back do we trust," decided *after*.
/// Same word ("select") would otherwise describe two different pipeline
/// stages if this trait kept its Phase 4b name (`RobustSelection`).
pub trait UpdateFilter: Send + Sync {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError>;
}

/// Filters a batch down to the updates `F` trusts, then hands the
/// survivors to `C` — any existing `Aggregator`, including `FedAvg` — to
/// combine. `F`'s filtering and `C`'s combining are independent choices;
/// composing them here (rather than one hardcoded `Aggregator` per
/// method) is what makes a future method needing a different combiner
/// over the same kind of filtering (or vice versa) free, not a new type.
pub struct FilteredAggregator<F: UpdateFilter, C: Aggregator> {
    filter: F,
    combiner: C,
}

impl<F: UpdateFilter, C: Aggregator> FilteredAggregator<F, C> {
    pub fn new(filter: F, combiner: C) -> Self {
        Self { filter, combiner }
    }
}

impl<F: UpdateFilter, C: Aggregator> Aggregator for FilteredAggregator<F, C> {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let selection = self.filter.filter(updates)?;
        if selection.selected_indices.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let selected: Vec<ClientDelta> = selection
            .selected_indices
            .iter()
            .map(|&i| updates[i].clone())
            .collect();
        self.combiner.aggregate(&selected)
    }
}

/// The assumed Byzantine count for a batch of `n`, clamped to always
/// leave at least one non-Byzantine update — a `cross_silo` round with 2
/// active clients can't meaningfully assume "20% Byzantine," and every
/// caller of this function degrades toward plain averaging rather than
/// erroring when `n` is too small for the formula to mean much.
fn byzantine_count(byzantine_fraction: f32, n: usize) -> usize {
    ((byzantine_fraction * n as f32).floor() as usize).min(n.saturating_sub(1))
}

/// Krum's score for every update: the sum of **squared** distances
/// (Blanchard et al., 2017's own definition) to its `n - f - 2` nearest
/// other updates, clamped to at least 1 neighbor so a tiny batch still
/// produces a well-defined (if less meaningful) score instead of
/// panicking.
fn krum_scores(
    byzantine_fraction: f32,
    updates: &[ClientDelta],
) -> Result<Vec<f32>, AggregatorError> {
    let n = updates.len();
    let distances = DistanceMatrix::from_updates(updates)?;
    let f = byzantine_count(byzantine_fraction, n);
    let neighbors_to_sum = n.saturating_sub(f + 2).clamp(1, n.saturating_sub(1).max(1));

    let mut scores = Vec::with_capacity(n);
    for i in 0..n {
        let mut dists: Vec<f32> = (0..n)
            .filter(|&j| j != i)
            .map(|j| distances.distance(i, j).powi(2))
            .collect();
        dists.sort_by(|a, b| a.partial_cmp(b).expect("distances are never NaN"));
        scores.push(dists.iter().take(neighbors_to_sum).sum());
    }
    Ok(scores)
}

/// Krum (Blanchard, El Mhamdi, Guerraoui & Stainer, 2017): keeps the
/// single lowest-scoring update — used directly as the round's new
/// weights (via `FilteredAggregator<_, FedAvg>`: averaging one item is a
/// no-op, so this reproduces Krum's own "use that one update" definition
/// exactly, without a separate single-item special case).
pub struct KrumFilter {
    pub byzantine_fraction: f32,
}

impl UpdateFilter for KrumFilter {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError> {
        let scores = krum_scores(self.byzantine_fraction, updates)?;
        let best = scores
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.partial_cmp(b.1).expect("scores are never NaN"))
            .map(|(i, _)| i)
            .expect("updates is non-empty, checked by FilteredAggregator before filter() runs");
        Ok(SelectionResult {
            selected_indices: vec![best],
        })
    }
}

/// Multi-Krum (same paper): same scoring as Krum, keeps the `n - f`
/// lowest-scoring updates instead of just one, then (via
/// `FilteredAggregator<_, FedAvg>`) combines them with this codebase's
/// existing sample-count-weighted mean — a documented choice, not every
/// presentation of Multi-Krum weights survivors this way, but every
/// other aggregator here already weights by `num_samples` and there's no
/// reason for this one to be the exception without a specific deployment
/// showing it should be (ADR 0008's "changing a default means
/// re-justifying against the literature," applied within a method).
pub struct MultiKrumFilter {
    pub byzantine_fraction: f32,
}

impl UpdateFilter for MultiKrumFilter {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError> {
        let scores = krum_scores(self.byzantine_fraction, updates)?;
        let n = updates.len();
        let f = byzantine_count(self.byzantine_fraction, n);
        let m = n.saturating_sub(f).max(1);

        let mut indices: Vec<usize> = (0..n).collect();
        indices.sort_by(|&a, &b| {
            scores[a]
                .partial_cmp(&scores[b])
                .expect("scores are never NaN")
        });
        indices.truncate(m);
        indices.sort_unstable(); // index order, not score order — deterministic, testable output

        Ok(SelectionResult {
            selected_indices: indices,
        })
    }
}

/// What varies about a coordinate-wise `robust` member: given every
/// client's value at one coordinate, combine them into one number.
/// `values` is `&mut` so an implementation can sort in place rather than
/// allocating its own copy.
pub trait CoordinateWiseRobustStatistic: Send + Sync {
    fn combine(&self, values_at_one_coordinate: &mut [f32]) -> f32;
}

/// Shared accumulation for coordinate-wise `robust` members (ADR 0002's
/// pattern, applied a second time within this family): decode every
/// update, then for each coordinate independently gather that
/// coordinate's value across every client and ask `S` to combine them.
/// Deliberately unweighted by `num_samples` — both cited methods
/// (Trimmed Mean, Median) are defined over equal-standing worker
/// vectors, not sample-weighted ones; a documented simplification, not a
/// silent omission.
pub struct CoordinateWiseAggregator<S: CoordinateWiseRobustStatistic> {
    statistic: S,
}

impl<S: CoordinateWiseRobustStatistic> CoordinateWiseAggregator<S> {
    pub fn new(statistic: S) -> Self {
        Self { statistic }
    }
}

impl<S: CoordinateWiseRobustStatistic> Aggregator for CoordinateWiseAggregator<S> {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let mut result = Vec::with_capacity(dim);
        for k in 0..dim {
            let mut column: Vec<f32> = decoded.iter().map(|w| w[k]).collect();
            result.push(self.statistic.combine(&mut column));
        }
        Ok(result)
    }
}

/// Coordinate-wise trimmed mean (Yin, Chen, Ramchandran & Bartlett,
/// 2018): sort each coordinate's values, drop the top/bottom
/// `byzantine_fraction`-derived count from each end, average what's
/// left. Clamped so at least one value always survives per coordinate.
pub struct TrimmedMeanStatistic {
    pub byzantine_fraction: f32,
}

impl CoordinateWiseRobustStatistic for TrimmedMeanStatistic {
    fn combine(&self, values: &mut [f32]) -> f32 {
        values.sort_by(|a, b| a.partial_cmp(b).expect("weights are never NaN"));
        let n = values.len();
        let trim = byzantine_count(self.byzantine_fraction, n).min(n.saturating_sub(1) / 2);
        let kept = &values[trim..n - trim];
        kept.iter().sum::<f32>() / kept.len() as f32
    }
}

/// Coordinate-wise median (same paper as `TrimmedMeanStatistic`): sort
/// each coordinate's values, take the middle one (or the average of the
/// two middle ones for an even-sized batch). No parameter — there's
/// nothing to tune.
pub struct MedianStatistic;

impl CoordinateWiseRobustStatistic for MedianStatistic {
    fn combine(&self, values: &mut [f32]) -> f32 {
        values.sort_by(|a, b| a.partial_cmp(b).expect("weights are never NaN"));
        let n = values.len();
        if n % 2 == 1 {
            values[n / 2]
        } else {
            (values[n / 2 - 1] + values[n / 2]) / 2.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::averaging::FedAvg;

    fn client_delta(client_id: &str, weights: &[f32]) -> ClientDelta {
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        ClientDelta {
            client_id: client_id.to_string(),
            round: 1,
            weights: bytes,
            num_samples: 1,
        }
    }

    // --- DistanceMatrix (unchanged behavior from Phase 4b) ---

    #[test]
    fn distance_to_self_is_zero() {
        let updates = vec![client_delta("a", &[1.0, 2.0])];
        let matrix = DistanceMatrix::from_updates(&updates).unwrap();

        assert_eq!(matrix.distance(0, 0), 0.0);
    }

    #[test]
    fn matrix_is_symmetric() {
        let updates = vec![
            client_delta("a", &[0.0, 0.0]),
            client_delta("b", &[3.0, 4.0]),
        ];
        let matrix = DistanceMatrix::from_updates(&updates).unwrap();

        assert_eq!(matrix.distance(0, 1), matrix.distance(1, 0));
    }

    #[test]
    fn known_two_point_distance_matches_hand_computed_l2() {
        let updates = vec![
            client_delta("a", &[0.0, 0.0]),
            client_delta("b", &[3.0, 4.0]),
        ];
        let matrix = DistanceMatrix::from_updates(&updates).unwrap();

        // classic 3-4-5 triangle
        assert!((matrix.distance(0, 1) - 5.0).abs() < 1e-5);
    }

    // --- Krum ---

    #[test]
    fn krum_excludes_an_obvious_outlier() {
        let aggregator = FilteredAggregator::new(
            KrumFilter {
                byzantine_fraction: 0.2,
            },
            FedAvg::default(),
        );
        let updates = vec![
            client_delta("honest-1", &[1.0, 1.0]),
            client_delta("honest-2", &[1.1, 0.9]),
            client_delta("honest-3", &[0.9, 1.1]),
            client_delta("attacker", &[1000.0, -1000.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!(
            result[0] < 2.0 && result[1] < 2.0,
            "result should stay near the honest cluster: {result:?}"
        );
    }

    #[test]
    fn krum_on_a_single_update_returns_it_unchanged() {
        let aggregator = FilteredAggregator::new(
            KrumFilter {
                byzantine_fraction: 0.2,
            },
            FedAvg::default(),
        );
        let updates = vec![client_delta("only", &[1.0, -2.0, 3.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![1.0, -2.0, 3.0]);
    }

    #[test]
    fn krum_empty_batch_errors() {
        let aggregator = FilteredAggregator::new(
            KrumFilter {
                byzantine_fraction: 0.2,
            },
            FedAvg::default(),
        );

        let err = match aggregator.aggregate(&[]) {
            Err(e) => e,
            Ok(_) => panic!("expected EmptyBatch"),
        };
        assert!(matches!(err, AggregatorError::EmptyBatch));
    }

    // --- Multi-Krum ---

    #[test]
    fn multi_krum_with_zero_byzantine_fraction_keeps_everyone() {
        let aggregator = FilteredAggregator::new(
            MultiKrumFilter {
                byzantine_fraction: 0.0,
            },
            FedAvg::default(),
        );
        let updates = vec![
            client_delta("a", &[1.0, 2.0]),
            client_delta("b", &[3.0, 4.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        // `byzantine_fraction = 0` => f = 0 => m = n => keeps both,
        // same-sample-count FedAvg mean of both.
        assert_eq!(result, vec![2.0, 3.0]);
    }

    #[test]
    fn multi_krum_excludes_a_minority_of_outliers() {
        let filter = MultiKrumFilter {
            byzantine_fraction: 0.3,
        };
        let updates = vec![
            client_delta("honest-1", &[1.0, 1.0]),
            client_delta("honest-2", &[1.1, 0.9]),
            client_delta("honest-3", &[0.9, 1.1]),
            client_delta("honest-4", &[1.0, 1.0]),
            client_delta("attacker", &[1000.0, -1000.0]),
        ];

        let selection = filter.filter(&updates).unwrap();

        assert!(
            !selection.selected_indices.contains(&4),
            "the attacker (index 4) should have been filtered out: {selection:?}"
        );
    }

    // --- Trimmed mean ---

    #[test]
    fn trimmed_mean_matches_hand_trimmed_result() {
        let aggregator = CoordinateWiseAggregator::new(TrimmedMeanStatistic {
            byzantine_fraction: 0.2,
        });
        // 5 clients, one coordinate: [1, 2, 3, 4, 100]. byzantine_fraction
        // 0.2 * 5 = 1 trimmed from each end -> kept [2, 3, 4] -> mean 3.
        let updates = vec![
            client_delta("a", &[1.0]),
            client_delta("b", &[2.0]),
            client_delta("c", &[3.0]),
            client_delta("d", &[4.0]),
            client_delta("e", &[100.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!((result[0] - 3.0).abs() < 1e-5, "got {result:?}");
    }

    #[test]
    fn trimmed_mean_resists_a_poisoned_minority() {
        let aggregator = CoordinateWiseAggregator::new(TrimmedMeanStatistic {
            byzantine_fraction: 0.25,
        });
        let updates = vec![
            client_delta("honest-1", &[1.0]),
            client_delta("honest-2", &[1.1]),
            client_delta("honest-3", &[0.9]),
            client_delta("honest-4", &[1.0]),
            client_delta("attacker", &[10_000.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!(
            result[0] < 2.0,
            "result pulled toward the attacker: {result:?}"
        );
    }

    // --- Median ---

    #[test]
    fn median_matches_known_middle_value_odd_count() {
        let aggregator = CoordinateWiseAggregator::new(MedianStatistic);
        let updates = vec![
            client_delta("a", &[5.0]),
            client_delta("b", &[1.0]),
            client_delta("c", &[3.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![3.0]);
    }

    #[test]
    fn median_averages_the_two_middle_values_even_count() {
        let aggregator = CoordinateWiseAggregator::new(MedianStatistic);
        let updates = vec![
            client_delta("a", &[1.0]),
            client_delta("b", &[2.0]),
            client_delta("c", &[3.0]),
            client_delta("d", &[4.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![2.5]);
    }

    #[test]
    fn median_resists_a_poisoned_minority() {
        let aggregator = CoordinateWiseAggregator::new(MedianStatistic);
        let updates = vec![
            client_delta("honest-1", &[1.0]),
            client_delta("honest-2", &[1.1]),
            client_delta("honest-3", &[0.9]),
            client_delta("attacker-1", &[10_000.0]),
            client_delta("attacker-2", &[-10_000.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!((result[0] - 1.0).abs() < 0.2, "got {result:?}");
    }

    // --- Small-batch clamping ---

    #[test]
    fn trimmed_mean_on_a_single_update_returns_it_unchanged() {
        let aggregator = CoordinateWiseAggregator::new(TrimmedMeanStatistic {
            byzantine_fraction: 0.2,
        });
        let updates = vec![client_delta("only", &[7.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![7.0]);
    }

    #[test]
    fn median_on_two_updates_averages_them() {
        let aggregator = CoordinateWiseAggregator::new(MedianStatistic);
        let updates = vec![client_delta("a", &[2.0]), client_delta("b", &[4.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![3.0]);
    }

    // --- Composability: FilteredAggregator accepts any Aggregator as its
    // combiner, not just FedAvg — the concrete claim this phase's
    // redesign makes (a future Bulyan-shaped method is a Krum-style
    // filter composed with a coordinate-wise combiner, with zero new
    // plumbing). Proven directly here, not just asserted in prose.

    #[test]
    fn filtered_aggregator_composes_with_a_non_fedavg_combiner() {
        // A Krum-style filter (keep the 1 closest-to-consensus update)
        // combined with a coordinate-wise median instead of FedAvg —
        // nobody ships this combination as a named strategy, but nothing
        // in `FilteredAggregator`'s definition prevents it, which is the
        // point.
        let aggregator = FilteredAggregator::new(
            MultiKrumFilter {
                byzantine_fraction: 0.2,
            },
            CoordinateWiseAggregator::new(MedianStatistic),
        );
        let updates = vec![
            client_delta("honest-1", &[1.0, 1.0]),
            client_delta("honest-2", &[1.1, 0.9]),
            client_delta("honest-3", &[0.9, 1.1]),
            client_delta("honest-4", &[1.0, 1.0]),
            client_delta("attacker", &[1000.0, -1000.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!(
            result[0] < 2.0 && result[1] < 2.0,
            "the attacker should have been filtered out before the median combiner even ran: {result:?}"
        );
    }
}
