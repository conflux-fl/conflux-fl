//! The `robust` (Byzantine-resilient) aggregation family (spec §5).
//!
//! Two composable shapes rather than one. The first pairs
//! `UpdateFilter` with `FilteredAggregator<F, C>`, for methods that pick
//! a *subset of whole updates* to keep (Krum, Multi-Krum). The second
//! pairs `CoordinateWiseRobustStatistic` with `CoordinateWiseAggregator<S>`,
//! for methods that combine *one coordinate at a time across every
//! client* (Trimmed Mean, Median) — these don't fit "selected whole
//! updates" at all, so forcing them through the first shape would
//! misrepresent what they compute.
//!
//! rationale, including why this split (rather than one shape, or two
//! unrelated ones) is what lets a future method needing *both* — e.g.
//! Bulyan, El Mhamdi, Guerraoui & Rouault (2018), *The Hidden
//! Vulnerability of Distributed Learning in Byzantine Settings*, ICML —
//! compose as `FilteredAggregator<SomeFilter,
//! CoordinateWiseAggregator<SomeStatistic>>` without changing anything in
//! this module.

use conflux_proto::ClientDelta;

use crate::weights::{accumulate_weighted, decode_and_validate};
use crate::{Aggregator, AggregatorError};

/// Pairwise L2 distances between a batch's decoded weight vectors — the
/// shared input Krum/Multi-Krum reason about (each update's score is a
/// function of its nearest neighbors). Trimmed Mean/Median never build
/// one — it's Krum/Multi-Krum-specific, not "robust family"-wide.
pub struct DistanceMatrix {
    distances: Vec<Vec<f32>>,
}

impl DistanceMatrix {
    /// Computes every pairwise L2 distance in the batch once.
    ///
    /// The selection-based members all need the same matrix, and it costs
    /// O(n² · dim) — computing it once and sharing it is why they can be
    /// small trait impls rather than whole aggregators.
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

    /// Distance between updates `i` and `j`. Symmetric.
    pub fn distance(&self, i: usize, j: usize) -> f32 {
        self.distances[i][j]
    }

    /// How many updates the matrix covers.
    pub fn len(&self) -> usize {
        self.distances.len()
    }

    /// Whether the matrix covers no updates.
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
    /// Indices into the original batch that survived filtering, in
    /// ascending order. Everything not listed was excluded.
    pub selected_indices: Vec<usize>,
}

/// What varies about which updates a selection-based `robust` member
/// trusts, given a batch. Deliberately distinct from
/// `conflux-selector::ClientSelector` — that trait answers "who trains
/// this round," decided *before* any update exists; this one answers
/// "which of the updates that came back do we trust," decided *after*.
/// Same word ("select") would otherwise describe two different pipeline
/// stages if this trait kept its name (`RobustSelection`).
pub trait UpdateFilter: Send + Sync {
    /// Chooses which updates survive. Excluding is the whole mechanism
    /// here — the surviving set is then combined by an ordinary
    /// aggregator, which is why these members compose.
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
    /// Pairs a filter with the aggregator that combines whatever survives
    /// it.
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
    /// Assumed fraction of the batch that may be Byzantine. Sizes how
    /// many updates this method excludes; too low lets an attacker
    /// through, too high discards honest clients.
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
    /// Assumed fraction of the batch that may be Byzantine, sizing how
    /// many updates are kept.
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
    /// Reduces one coordinate's values, across every client, to a single
    /// number. Called once per coordinate; the slice is scratch space the
    /// implementation may reorder.
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
    /// Builds an aggregator that applies `statistic` coordinate by
    /// coordinate.
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
    /// Assumed Byzantine fraction, sizing how many values are trimmed
    /// from each end of each coordinate.
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

/// Coordinate-wise median-of-means (Chen, Su & Xu, 2017, *Distributed
/// Statistical Machine Learning in Adversarial Settings: Byzantine
/// Gradient Descent*, ACM SIGMETRICS/POMACS 2017): partition the batch
/// into fixed-size groups (by array position — the same position always
/// belongs to the same client across every coordinate, so this needs no
/// extra bookkeeping to stay consistent coordinate-to-coordinate), average
/// within each group, then take the median *of those group means*. A
/// single attacker only ever corrupts the one group it lands in, so it
/// can pull that one group's mean but not the overall median across
/// groups — a different robustness mechanism from trimming or taking the
/// raw median directly, worth having as its own citable member rather
/// than treating it as a variant of `MedianStatistic`.
pub struct MedianOfMeansStatistic {
    /// How many clients per group. Coordinates are reduced within each
    /// group first, then across groups.
    pub group_size: usize,
}

impl Default for MedianOfMeansStatistic {
    fn default() -> Self {
        // Pairs: the smallest group size where "average within a group"
        // does anything at all (group_size=1 degenerates to plain
        // `MedianStatistic`). A deployer wanting fewer, larger groups
        // constructs this directly rather than through the config-driven
        // registry, same as `GeometricMedianStatistic`'s iteration count.
        Self { group_size: 2 }
    }
}

impl CoordinateWiseRobustStatistic for MedianOfMeansStatistic {
    fn combine(&self, values: &mut [f32]) -> f32 {
        let group_size = self.group_size.max(1);
        let mut group_means: Vec<f32> = values
            .chunks(group_size)
            .map(|chunk| chunk.iter().sum::<f32>() / chunk.len() as f32)
            .collect();
        group_means.sort_by(|a, b| a.partial_cmp(b).expect("weights are never NaN"));
        let n = group_means.len();
        if n % 2 == 1 {
            group_means[n / 2]
        } else {
            (group_means[n / 2 - 1] + group_means[n / 2]) / 2.0
        }
    }
}

/// FABA (Xia, Zhang, Yang, Shao & Yin, 2019, *FABA: An Algorithm for Fast
/// Aggregation against Byzantine Attacks in Distributed Neural Networks*,
/// IJCAI 2019): iteratively removes whichever update is farthest from the
/// mean of whatever currently remains, `f` times, then hands the
/// survivors to a combiner (`FedAvg` here — the same documented
/// sample-count-weighted-combiner modeling choice as Multi-Krum's own
/// combiner, ADR 0008). Simpler than Krum's pairwise-distance scoring —
/// FABA's whole pitch is speed — at the cost of only ever removing one
/// point per pass instead of scoring everyone against everyone.
pub struct FabaFilter {
    /// Assumed Byzantine fraction, sizing how many updates are dropped.
    pub byzantine_fraction: f32,
}

impl UpdateFilter for FabaFilter {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError> {
        let decoded = decode_and_validate(updates)?;
        let n = decoded.len();
        let f = byzantine_count(self.byzantine_fraction, n);

        let mut remaining: Vec<usize> = (0..n).collect();
        for _ in 0..f {
            if remaining.len() <= 1 {
                break;
            }
            let dim = decoded[remaining[0]].len();
            let mut mean = vec![0.0f32; dim];
            // `1/n` per term rather than after summing: two clients near
            // `f32::MAX` overflow the running total to infinity, and
            // dividing infinity by the count leaves it infinite. Same
            // defect as the geometric median's, same fix.
            let share = 1.0 / remaining.len() as f32;
            for &i in &remaining {
                accumulate_weighted(&mut mean, &decoded[i], share);
            }

            let (worst_pos, _) = remaining
                .iter()
                .enumerate()
                .map(|(pos, &i)| (pos, l2_distance(&decoded[i], &mean)))
                .max_by(|a, b| a.1.partial_cmp(&b.1).expect("distances are never NaN"))
                .expect("remaining is non-empty, checked by the loop condition above");
            remaining.remove(worst_pos);
        }

        remaining.sort_unstable();
        Ok(SelectionResult {
            selected_indices: remaining,
        })
    }
}

/// Bulyan (El Mhamdi, Guerraoui & Rouault, 2018, *The Hidden
/// Vulnerability of Distributed Learning in Byzantium*, ICML 2018):
/// repeatedly applies Krum's own scoring to a shrinking pool — removing
/// the single best-scoring update and adding it to a "selection set" each
/// time — until `n - 2f` updates have been selected. Where a single Krum
/// or Multi-Krum pass can be fooled by an attacker crafted to look
/// consistently "central" across the whole batch, re-scoring the
/// remaining pool from scratch after every removal is what the paper
/// shows closes that gap.
///
/// **Documented modeling choice**: the paper's combine step trims exactly
/// `2f` values (computed from the *original* batch size `n`) from each
/// end of the selected set before averaging. `FilteredAggregator`'s
/// combiner only ever sees the already-filtered survivors — not the
/// original `n` — so reproducing that exact count would need new
/// plumbing threading extra state from filter to combiner. Composing
/// `BulyanFilter` with the existing `CoordinateWiseAggregator<
/// TrimmedMeanStatistic>` instead (trimming `byzantine_fraction` of the
/// *selected* set's size) keeps this a zero-new-architecture composition,
/// consistent with Multi-Krum's own documented combiner simplification
/// (ADR 0008) — the selection mechanism, which is Bulyan's actual novel
/// contribution over Multi-Krum, is unmodified and exact.
pub struct BulyanFilter {
    /// Assumed Byzantine fraction, sizing the selection step.
    pub byzantine_fraction: f32,
}

impl UpdateFilter for BulyanFilter {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError> {
        let n = updates.len();
        let f = byzantine_count(self.byzantine_fraction, n);
        let theta = n.saturating_sub(2 * f).max(1);

        let mut remaining: Vec<usize> = (0..n).collect();
        let mut selected: Vec<usize> = Vec::with_capacity(theta);

        while selected.len() < theta && remaining.len() > 1 {
            let pool: Vec<ClientDelta> = remaining.iter().map(|&i| updates[i].clone()).collect();
            let scores = krum_scores(self.byzantine_fraction, &pool)?;
            let (best_pos, _) = scores
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.partial_cmp(b.1).expect("scores are never NaN"))
                .expect("pool is non-empty, checked by the loop condition");
            selected.push(remaining.remove(best_pos));
        }
        if selected.len() < theta {
            selected.extend(remaining);
        }

        selected.sort_unstable();
        Ok(SelectionResult {
            selected_indices: selected,
        })
    }
}

/// Top right singular vector of a centered `n x dim` matrix (rows =
/// `centered`), via power iteration on `U^T U` — computed as alternating
/// `U^T(U v)` products rather than ever forming the `dim x dim` matrix
/// explicitly, since `dim` can be tens of thousands (a real model's flat
/// parameter count) while `n` (client count per round) stays small.
/// Deterministic uniform start (not random) so callers get reproducible
/// output — reasonable as long as the start isn't exactly orthogonal to
/// the true top direction, a measure-zero case for real data.
fn top_singular_vector(centered: &[Vec<f32>], iterations: usize) -> Vec<f32> {
    let dim = centered[0].len();
    let mut v = vec![1.0f32 / (dim as f32).sqrt(); dim];

    for _ in 0..iterations {
        let projections: Vec<f32> = centered
            .iter()
            .map(|row| row.iter().zip(&v).map(|(a, b)| a * b).sum())
            .collect();

        let mut next = vec![0.0f32; dim];
        for (row, &p) in centered.iter().zip(&projections) {
            for (n, r) in next.iter_mut().zip(row) {
                *n += p * r;
            }
        }

        let norm = next.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 1e-12 {
            for n in &mut next {
                *n /= norm;
            }
            v = next;
        } else {
            break; // degenerate (all updates identical) — keep the last v
        }
    }
    v
}

/// Divide-and-Conquer (Shejwalkar & Houmansadr, 2021, *Manipulating the
/// Byzantine: Optimizing Model Poisoning Attacks and Defenses for
/// Federated Learning*, NDSS 2021): scores each update by its squared
/// projection onto the batch's top principal direction (the direction of
/// greatest variance across the centered updates), removes the `f`
/// highest-scoring — catching attacks a distance-based method like Krum
/// can miss, since a coordinated attack designed to look individually
/// close to *some* other updates can still stand out as the dominant
/// source of variance across the whole batch.
///
/// **Documented modeling choice**: the paper's full algorithm optionally
/// subsamples a random subset of dimensions per iteration (a performance
/// optimization for very high-dimensional models) and repeats over
/// several iterations with a union-of-flagged-clients rule. This
/// implementation is the paper's own `b = full dimensionality, niters =
/// 1` special case — strictly more thorough per pass, not a weaker
/// approximation — since Conflux's model sizes in practice don't need the
/// subsampling optimization, and skipping it keeps this deterministic
/// (no RNG, no per-run variance in what a test asserts).
pub struct DivideAndConquerFilter {
    /// Assumed Byzantine fraction, sizing how many updates the spectral
    /// score excludes.
    pub byzantine_fraction: f32,
    /// Power iteration count for the top singular vector. The
    /// deterministic start converges quickly for well-separated data; 30
    /// is generous headroom, not a tuned minimum.
    pub power_iterations: usize,
}

impl Default for DivideAndConquerFilter {
    fn default() -> Self {
        Self {
            byzantine_fraction: 0.2,
            power_iterations: 30,
        }
    }
}

impl UpdateFilter for DivideAndConquerFilter {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError> {
        let decoded = decode_and_validate(updates)?;
        let n = decoded.len();
        let f = byzantine_count(self.byzantine_fraction, n);
        if f == 0 {
            return Ok(SelectionResult {
                selected_indices: (0..n).collect(),
            });
        }

        let dim = decoded[0].len();
        let mut mean = vec![0.0f32; dim];
        // Normalized per term — summing first overflows to infinity on a
        // batch of large-but-finite weights, and the centering below
        // would then subtract infinity from every row.
        let share = 1.0 / n as f32;
        for row in &decoded {
            accumulate_weighted(&mut mean, row, share);
        }
        let centered: Vec<Vec<f32>> = decoded
            .iter()
            .map(|row| row.iter().zip(&mean).map(|(x, m)| x - m).collect())
            .collect();

        let v = top_singular_vector(&centered, self.power_iterations);
        let mut scores: Vec<(usize, f32)> = centered
            .iter()
            .enumerate()
            .map(|(i, row)| {
                let projection: f32 = row.iter().zip(&v).map(|(a, b)| a * b).sum();
                (i, projection * projection)
            })
            .collect();
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).expect("scores are never NaN"));

        let excluded: std::collections::HashSet<usize> =
            scores.iter().take(f).map(|&(i, _)| i).collect();
        let selected: Vec<usize> = (0..n).filter(|i| !excluded.contains(i)).collect();

        Ok(SelectionResult {
            selected_indices: selected,
        })
    }
}

/// What a whole-vector `robust` member computes over an entire batch at
/// once, rather than one coordinate at a time — needed for a method whose
/// output isn't equal to combining each coordinate independently, unlike
/// `CoordinateWiseRobustStatistic`'s assumption that coordinates never
/// interact. Takes `(weight, decoded_weights)` pairs directly (rather
/// than raw `ClientDelta`s) so an implementation never has to decode or
/// know about the wire format itself.
pub trait RobustVectorStatistic: Send + Sync {
    /// Reduces the whole batch to one vector at once, rather than
    /// coordinate by coordinate — for statistics like the geometric
    /// median that are defined over whole vectors and cannot be
    /// decomposed per coordinate.
    ///
    /// Weights arrive as raw sample counts; normalize before accumulating
    /// to avoid overflow.
    fn combine(&self, weighted_updates: &[(f32, Vec<f32>)]) -> Vec<f32>;
}

/// Shared accumulation for whole-vector `robust` members (ADR 0002's
/// pattern, applied a third time within this family): decode every
/// update, pair each with its `num_samples` as a weight, hand the whole
/// batch to `S` at once.
pub struct VectorRobustAggregator<S: RobustVectorStatistic> {
    statistic: S,
}

impl<S: RobustVectorStatistic> VectorRobustAggregator<S> {
    /// Builds an aggregator around a whole-vector statistic.
    pub fn new(statistic: S) -> Self {
        Self { statistic }
    }
}

impl<S: RobustVectorStatistic> Aggregator for VectorRobustAggregator<S> {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let weighted: Vec<(f32, Vec<f32>)> = updates
            .iter()
            .zip(decoded)
            .map(|(u, w)| (u.num_samples as f32, w))
            .collect();
        Ok(self.statistic.combine(&weighted))
    }
}

/// Weighted geometric median via Weiszfeld's algorithm (Pillutla, Kakade
/// & Harchaoui, *Robust Aggregation for Federated Learning*, IEEE
/// Transactions on Signal Processing 2022, first appeared as arXiv 2019 —
/// "RFA"): the point minimizing the sum of weighted L2 distances to every
/// update in the batch. Unlike `TrimmedMeanStatistic`/`MedianStatistic`
/// (deliberately unweighted in this crate — a documented simplification
/// against their own paper's equal-standing-worker definition), RFA's
/// own paper explicitly defines a *weighted* geometric median for the FL
/// setting — this implementation follows that directly, weighting by
/// `num_samples` like every other aggregator in this codebase, not a
/// simplification here.
///
/// Combines every coordinate jointly rather than independently, which is
/// what makes it rotation-invariant and able to use cross-coordinate
/// structure a coordinate-wise statistic discards — the reason this
/// needed `RobustVectorStatistic` rather than fitting
/// `CoordinateWiseRobustStatistic`.
pub struct GeometricMedianStatistic {
    /// Weiszfeld's algorithm converges iteratively rather than in closed
    /// form; a fixed iteration count (rather than a convergence-tolerance
    /// check) keeps the result deterministic and simple to test. The RFA
    /// paper reports most of the robustness benefit within a handful of
    /// iterations in practice.
    pub iterations: usize,
}

impl Default for GeometricMedianStatistic {
    fn default() -> Self {
        Self { iterations: 8 }
    }
}

impl RobustVectorStatistic for GeometricMedianStatistic {
    fn combine(&self, weighted_updates: &[(f32, Vec<f32>)]) -> Vec<f32> {
        let dim = weighted_updates[0].1.len();

        // Weiszfeld needs a starting estimate; the weighted arithmetic
        // mean is the standard choice (never itself equal to an outlier,
        // so the first reweighting step is always well-defined).
        //
        // Weights are normalized *before* accumulating, not after. The
        // result is identical — a weighted mean is scale-invariant in
        // its weights — but the intermediate is not: these weights
        // arrive as raw sample counts, so `w * x` for a client reporting
        // 10 samples and a weight near `f32::MAX` overflows to infinity
        // before the division that would have brought it back. That
        // infinity then propagates through every Weiszfeld iteration and
        // into the checkpoint. `WeightedAverageAggregator` already
        // normalizes first for the same reason; this is the same
        // ordering, not a different formula.
        let mut estimate = vec![0.0f32; dim];
        let total_weight: f32 = weighted_updates.iter().map(|(w, _)| w).sum();
        if total_weight > 0.0 {
            for (w, v) in weighted_updates {
                let normalized = w / total_weight;
                for (e, x) in estimate.iter_mut().zip(v) {
                    *e += normalized * x;
                }
            }
        }

        // Guards division-by-zero when the estimate lands exactly on one
        // of the updates (common once it converges close to a cluster).
        const EPS: f32 = 1e-6;
        for _ in 0..self.iterations {
            // Two passes for the same reason the initialization above
            // normalizes first: the inverse-distance weights can be
            // large (`w / EPS` is 1e7 for a client sitting on the
            // current estimate), and multiplying that by a large
            // coordinate overflows. Summing them first costs one extra
            // traversal and makes the accumulation bounded by
            // construction.
            let inverse_distance_weights: Vec<f32> = weighted_updates
                .iter()
                .map(|(w, v)| w / l2_distance(v, &estimate).max(EPS))
                .collect();
            let weight_sum: f32 = inverse_distance_weights.iter().sum();
            if weight_sum <= 0.0 || !weight_sum.is_finite() {
                // Every update is infinitely far from the estimate, or
                // the weights themselves overflowed. Neither leaves a
                // meaningful direction to step in, so hold the current
                // estimate rather than stepping to a garbage one.
                break;
            }

            let mut next = vec![0.0f32; dim];
            for (inverse_distance_weight, (_, v)) in
                inverse_distance_weights.iter().zip(weighted_updates)
            {
                let normalized = inverse_distance_weight / weight_sum;
                for (s, x) in next.iter_mut().zip(v) {
                    *s += normalized * x;
                }
            }
            estimate = next;
        }
        estimate
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
            ..Default::default()
        }
    }

    // --- DistanceMatrix (unchanged behavior from) ---

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

    // --- FABA ---

    #[test]
    fn faba_excludes_an_obvious_outlier() {
        // FABA removes exactly `f = floor(byzantine_fraction * n)` points,
        // unlike Krum (which always keeps exactly 1 regardless of `f`) —
        // 0.3 * 4 = 1, so this actually triggers a removal.
        let aggregator = FilteredAggregator::new(
            FabaFilter {
                byzantine_fraction: 0.3,
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
    fn faba_on_a_single_update_returns_it_unchanged() {
        let aggregator = FilteredAggregator::new(
            FabaFilter {
                byzantine_fraction: 0.2,
            },
            FedAvg::default(),
        );
        let updates = vec![client_delta("only", &[4.0, -1.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![4.0, -1.0]);
    }

    #[test]
    fn faba_resists_a_poisoned_minority() {
        let aggregator = FilteredAggregator::new(
            FabaFilter {
                byzantine_fraction: 0.2,
            },
            FedAvg::default(),
        );
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

    // --- Bulyan ---

    #[test]
    fn bulyan_excludes_an_obvious_outlier() {
        let filter = BulyanFilter {
            byzantine_fraction: 0.15,
        };
        // n=9, f=1, theta=7: enough room for one obvious attacker to be
        // excluded by the iterated-Krum selection before the combine step.
        let updates = vec![
            client_delta("honest-1", &[1.0]),
            client_delta("honest-2", &[1.05]),
            client_delta("honest-3", &[0.95]),
            client_delta("honest-4", &[1.0]),
            client_delta("honest-5", &[1.02]),
            client_delta("honest-6", &[0.98]),
            client_delta("honest-7", &[1.0]),
            client_delta("honest-8", &[1.01]),
            client_delta("attacker", &[10_000.0]),
        ];

        let selection = filter.filter(&updates).unwrap();

        assert!(
            !selection.selected_indices.contains(&8),
            "the attacker (index 8) should have been excluded by iterated Krum selection: {selection:?}"
        );
    }

    #[test]
    fn bulyan_resists_a_poisoned_minority_end_to_end() {
        let aggregator = FilteredAggregator::new(
            BulyanFilter {
                byzantine_fraction: 0.15,
            },
            CoordinateWiseAggregator::new(TrimmedMeanStatistic {
                byzantine_fraction: 0.15,
            }),
        );
        let updates = vec![
            client_delta("honest-1", &[1.0]),
            client_delta("honest-2", &[1.05]),
            client_delta("honest-3", &[0.95]),
            client_delta("honest-4", &[1.0]),
            client_delta("honest-5", &[1.02]),
            client_delta("honest-6", &[0.98]),
            client_delta("honest-7", &[1.0]),
            client_delta("honest-8", &[1.01]),
            client_delta("attacker", &[10_000.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!(
            (result[0] - 1.0).abs() < 0.2,
            "result pulled toward the attacker: {result:?}"
        );
    }

    #[test]
    fn bulyan_on_a_single_update_returns_it_unchanged() {
        let aggregator = FilteredAggregator::new(
            BulyanFilter {
                byzantine_fraction: 0.15,
            },
            CoordinateWiseAggregator::new(TrimmedMeanStatistic {
                byzantine_fraction: 0.15,
            }),
        );
        let updates = vec![client_delta("only", &[3.0, -2.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![3.0, -2.0]);
    }

    // --- Geometric median ---

    #[test]
    fn geometric_median_of_three_colinear_points_matches_their_median() {
        // For colinear, equally-weighted points, the geometric median
        // degenerates to the classic 1D median — a known closed form to
        // check convergence against.
        let aggregator = VectorRobustAggregator::new(GeometricMedianStatistic::default());
        let updates = vec![
            client_delta("a", &[0.0]),
            client_delta("b", &[1.0]),
            client_delta("c", &[3.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!((result[0] - 1.0).abs() < 1e-3, "got {result:?}");
    }

    #[test]
    fn geometric_median_resists_a_poisoned_minority() {
        let aggregator = VectorRobustAggregator::new(GeometricMedianStatistic::default());
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

    #[test]
    fn geometric_median_weights_by_num_samples() {
        // A dominant weight (> sum of every other weight) pins the
        // weighted geometric median exactly at that point — a known,
        // exactly-checkable property, and direct proof this
        // implementation actually uses `num_samples`, not an unweighted
        // geometric median that would land near the segment's middle.
        let statistic = GeometricMedianStatistic { iterations: 50 };
        let light = (1.0f32, vec![0.0f32]);
        let heavy = (10.0f32, vec![10.0f32]);

        let result = statistic.combine(&[light, heavy]);

        assert!((result[0] - 10.0).abs() < 0.1, "got {result:?}");
    }

    #[test]
    fn geometric_median_on_a_single_update_returns_it_unchanged() {
        let aggregator = VectorRobustAggregator::new(GeometricMedianStatistic::default());
        let updates = vec![client_delta("only", &[5.0, -3.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![5.0, -3.0]);
    }

    // --- Median of means ---

    #[test]
    fn median_of_means_matches_hand_computed_result() {
        let aggregator = CoordinateWiseAggregator::new(MedianOfMeansStatistic { group_size: 2 });
        // pairs -> group means [2, 101, 6] -> sorted [2, 6, 101] -> median 6
        let updates = vec![
            client_delta("a", &[1.0]),
            client_delta("b", &[3.0]),
            client_delta("c", &[100.0]),
            client_delta("d", &[102.0]),
            client_delta("e", &[5.0]),
            client_delta("f", &[7.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!((result[0] - 6.0).abs() < 1e-5, "got {result:?}");
    }

    #[test]
    fn median_of_means_resists_a_single_poisoned_group() {
        let aggregator = CoordinateWiseAggregator::new(MedianOfMeansStatistic { group_size: 2 });
        // groups: [1,1]->1.0, [1,1]->1.0, [1,10000]->5000.5 -> median of
        // the three group means is 1.0 — only one group is corrupted.
        let updates = vec![
            client_delta("honest-1", &[1.0]),
            client_delta("honest-2", &[1.0]),
            client_delta("honest-3", &[1.0]),
            client_delta("honest-4", &[1.0]),
            client_delta("honest-5", &[1.0]),
            client_delta("attacker", &[10_000.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!((result[0] - 1.0).abs() < 1e-3, "got {result:?}");
    }

    #[test]
    fn median_of_means_on_a_single_update_returns_it_unchanged() {
        let aggregator = CoordinateWiseAggregator::new(MedianOfMeansStatistic { group_size: 2 });
        let updates = vec![client_delta("only", &[9.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![9.0]);
    }

    // --- Divide and conquer ---

    #[test]
    fn divide_and_conquer_excludes_the_dominant_variance_outlier() {
        let filter = DivideAndConquerFilter {
            byzantine_fraction: 0.2,
            ..Default::default()
        };
        // dim1 is constant across every update (zero variance there), so
        // the top singular direction is exactly dim0 — the attacker's
        // huge dim0 deviation makes it the obvious highest-scoring point.
        let updates = vec![
            client_delta("honest-1", &[-0.1, 5.0]),
            client_delta("honest-2", &[0.0, 5.0]),
            client_delta("honest-3", &[0.1, 5.0]),
            client_delta("honest-4", &[0.0, 5.0]),
            client_delta("attacker", &[1000.0, 5.0]),
        ];

        let selection = filter.filter(&updates).unwrap();

        assert!(
            !selection.selected_indices.contains(&4),
            "the attacker (index 4) should have been excluded: {selection:?}"
        );
    }

    #[test]
    fn divide_and_conquer_resists_a_poisoned_minority_end_to_end() {
        let aggregator = FilteredAggregator::new(
            DivideAndConquerFilter {
                byzantine_fraction: 0.2,
                ..Default::default()
            },
            FedAvg::default(),
        );
        let updates = vec![
            client_delta("honest-1", &[-0.1, 5.0]),
            client_delta("honest-2", &[0.0, 5.0]),
            client_delta("honest-3", &[0.1, 5.0]),
            client_delta("honest-4", &[0.0, 5.0]),
            client_delta("attacker", &[1000.0, 5.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        assert!(
            result[0].abs() < 1.0 && (result[1] - 5.0).abs() < 1e-3,
            "result pulled toward the attacker: {result:?}"
        );
    }

    #[test]
    fn divide_and_conquer_with_zero_byzantine_fraction_keeps_everyone() {
        let filter = DivideAndConquerFilter {
            byzantine_fraction: 0.2,
            ..Default::default()
        };
        // 0.2 * 3 = 0 (floored) -> nothing removed.
        let updates = vec![
            client_delta("a", &[1.0]),
            client_delta("b", &[2.0]),
            client_delta("c", &[3.0]),
        ];

        let selection = filter.filter(&updates).unwrap();

        assert_eq!(selection.selected_indices, vec![0, 1, 2]);
    }
}
