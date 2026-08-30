//! Aggregation algorithms (family-based): turns one round's batch of
//! client-submitted weight updates into the new global model weights.
//! Every shipped method belongs to one of three families — `averaging`,
//! `robust` (Byzantine-resilient), and `temporal` (cross-round,
//! history-aware) — each built around one small trait capturing what a
//! family member varies, so a new method is typically a short trait impl
//! reusing the family's existing accumulation logic rather than a whole
//! new implementation written from scratch.

mod averaging;
mod robust;
mod temporal;
mod weights;

pub use averaging::{AveragingWeighting, FedAvg, SampleCountWeighting, WeightedAverageAggregator};
pub use robust::{
    BulyanFilter, CoordinateWiseAggregator, CoordinateWiseRobustStatistic, DistanceMatrix,
    DivideAndConquerFilter, FabaFilter, FilteredAggregator, GeometricMedianStatistic, KrumFilter,
    MedianOfMeansStatistic, MedianStatistic, MultiKrumFilter, RobustVectorStatistic,
    SelectionResult, TrimmedMeanStatistic, UpdateFilter, VectorRobustAggregator,
};
pub use temporal::{
    CenteredClippingAggregator, ClientDssDiagnostic, DssAggregator, FoolsGoldAggregator,
};

use conflux_config::{StrategyEntry, StrategyKind};
use conflux_proto::ClientDelta;

/// Turns one round's batch of client updates into new global weights.
/// Every aggregation family member implements this: `FedAvg` directly;
/// selection-based `robust` members (`Krum`, `Multi-Krum`, `FABA`,
/// `Bulyan`, `Divide-and-Conquer`) via `FilteredAggregator`;
/// coordinate-wise `robust` members (`Trimmed Mean`, `Median`,
/// `Median-of-Means`) via `CoordinateWiseAggregator`; whole-vector
/// `robust` members (`Geometric Median`) via `VectorRobustAggregator`;
/// and history-aware `temporal` members (`FoolsGold`, `Centered
/// Clipping`, and the research-only `DssAggregator`) with their own
/// internal state. Conflux's aim is a
/// faithful, extensible catalog of published methods for researchers to
/// compare against — see each type's own doc comment for its citation;
/// adding another method is a new small trait impl composed with the
/// existing shared accumulators, not a change to this trait or
/// `Aggregator` itself.
pub trait Aggregator: Send + Sync {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError>;
}

// Registers every shipped, config-selectable family member into
// `conflux-config`'s compile-time strategy registry — `build_aggregator`
// is what actually turns a config-resolved name into a constructed
// `Aggregator`; these submissions are what let `conflux_config::lookup`
// find each name at all, independent of whether anything ever calls
// `build_aggregator`. `DssAggregator` (see `temporal.rs`) deliberately
// has no entry here — it's an unvalidated research method, constructed
// directly by whoever wants to run it, not selectable via a config
// string.
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "fedavg" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "krum" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "multi_krum" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "trimmed_mean" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "median" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "faba" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "bulyan" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "geometric_median" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "median_of_means" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "divide_and_conquer" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "foolsgold" }
}
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "centered_clipping" }
}

#[derive(Debug, thiserror::Error)]
pub enum AggregatorBuildError {
    #[error(
        "unknown aggregator \"{0}\" — not a registered conflux-core strategy \
         (known: \"fedavg\", \"krum\", \"multi_krum\", \"trimmed_mean\", \"median\", \
         \"faba\", \"bulyan\", \"geometric_median\", \"median_of_means\", \
         \"divide_and_conquer\", \"foolsgold\", \"centered_clipping\")"
    )]
    Unknown(String),
}

/// The algorithm-tuning values `build_aggregator` needs, gathered into
/// one struct rather than a growing list of positional `f32`s.
///
/// Each field is read by exactly one family and ignored by the others,
/// so most call sites care about one of them — hence `Default` plus
/// struct-update syntax (`AggregatorParams { clip_radius: 2.0,
/// ..Default::default() }`), which also keeps two same-typed knobs from
/// being silently transposable at a call site the way two bare `f32`
/// arguments would be.
///
/// The defaults match `conflux-config`'s own builtin fallbacks for the
/// corresponding fields, so constructing an aggregator without a
/// resolved config gives the same behavior a config with nothing
/// overridden would.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AggregatorParams {
    /// The assumed fraction of a round's batch that might be Byzantine,
    /// used to size how many updates each method excludes or trims:
    /// Krum's *f*, Multi-Krum's *m*, Trimmed Mean's trim count. Read
    /// only by the `robust` family; `fedavg` ignores it entirely.
    pub byzantine_fraction: f32,
    /// Centered Clipping's `τ` — the radius any one client's deviation
    /// from the running reference is clipped to. Read only by
    /// `centered_clipping`. Problem-scale dependent: see
    /// [`CenteredClippingAggregator`]'s own fidelity notes.
    pub clip_radius: f32,
}

impl Default for AggregatorParams {
    fn default() -> Self {
        Self {
            byzantine_fraction: 0.2,
            clip_radius: 1.0,
        }
    }
}

/// Constructs the `Aggregator` named by a resolved `config.aggregator.value`.
/// Every match arm mirrors one `inventory::submit!` above — adding a
/// family member means adding both, not restructuring this function.
pub fn build_aggregator(
    name: &str,
    params: AggregatorParams,
) -> Result<Box<dyn Aggregator>, AggregatorBuildError> {
    let AggregatorParams {
        byzantine_fraction,
        clip_radius,
    } = params;
    match name {
        "fedavg" => Ok(Box::new(FedAvg::default())),
        "krum" => Ok(Box::new(FilteredAggregator::new(
            KrumFilter { byzantine_fraction },
            FedAvg::default(),
        ))),
        "multi_krum" => Ok(Box::new(FilteredAggregator::new(
            MultiKrumFilter { byzantine_fraction },
            FedAvg::default(),
        ))),
        "trimmed_mean" => Ok(Box::new(CoordinateWiseAggregator::new(
            TrimmedMeanStatistic { byzantine_fraction },
        ))),
        "median" => Ok(Box::new(CoordinateWiseAggregator::new(MedianStatistic))),
        "faba" => Ok(Box::new(FilteredAggregator::new(
            FabaFilter { byzantine_fraction },
            FedAvg::default(),
        ))),
        "bulyan" => Ok(Box::new(FilteredAggregator::new(
            BulyanFilter { byzantine_fraction },
            CoordinateWiseAggregator::new(TrimmedMeanStatistic { byzantine_fraction }),
        ))),
        "geometric_median" => Ok(Box::new(VectorRobustAggregator::new(
            GeometricMedianStatistic::default(),
        ))),
        "median_of_means" => Ok(Box::new(CoordinateWiseAggregator::new(
            MedianOfMeansStatistic::default(),
        ))),
        "divide_and_conquer" => Ok(Box::new(FilteredAggregator::new(
            DivideAndConquerFilter {
                byzantine_fraction,
                ..Default::default()
            },
            FedAvg::default(),
        ))),
        "foolsgold" => Ok(Box::new(FoolsGoldAggregator::default())),
        "centered_clipping" => Ok(Box::new(CenteredClippingAggregator::new(clip_radius))),
        other => Err(AggregatorBuildError::Unknown(other.to_string())),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AggregatorError {
    #[error("cannot aggregate an empty batch of updates")]
    EmptyBatch,
    /// A client submitted `NaN` or an infinity.
    ///
    /// This is a rejection, not a repair, and it is checked before any
    /// aggregator sees the batch — because the alternative is not
    /// "slightly wrong output". Six of the shipped aggregators sort or
    /// compare their inputs (`partial_cmp` returns `None` for `NaN`) and
    /// **panicked**, taking the server down; the other six propagated
    /// `NaN` into the checkpoint, where it persists forever because every
    /// subsequent round starts from it. Either way a single client, with
    /// four bytes, ends the experiment.
    ///
    /// Nothing on the wire prevents this: `decode_weights` accepts any
    /// 4-byte pattern, as it must — the codec has no way to know which
    /// bit patterns are meaningful for a given model.
    #[error(
        "update {client_id} contains a non-finite weight (NaN or infinity)          at index {index}"
    )]
    NonFiniteWeights { client_id: String, index: usize },
    /// A client reported a sample count that cannot be a real one.
    ///
    /// `num_samples` is self-reported and unauthenticated, and FedAvg
    /// weights by it directly (McMahan et al. 2017 assumes honest
    /// counts). This check only rejects counts that are *structurally*
    /// impossible — see [`MAX_PLAUSIBLE_SAMPLE_COUNT`]. It is not a
    /// defense against a client that merely exaggerates within
    /// plausible bounds; see that constant's docs for what is.
    #[error(
        "update {client_id} reports {got} samples, which exceeds the largest          plausible count ({max}) — self-reported sample counts are          unauthenticated and this one cannot be real"
    )]
    ImplausibleSampleCount {
        client_id: String,
        got: u64,
        max: u64,
    },
    #[error(
        "update {client_id} has {got} weights, expected {expected} \
         (every update in a batch must have the same weight-vector length)"
    )]
    MismatchedLength {
        client_id: String,
        expected: usize,
        got: usize,
    },
    #[error("update {client_id} has malformed weights: {len} bytes is not a multiple of 4")]
    MalformedWeights { client_id: String, len: usize },
    #[error("batch weights sum to zero — cannot normalize")]
    ZeroWeightSum,
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAMES: &[&str] = &[
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

    #[test]
    fn build_aggregator_succeeds_for_every_shipped_name() {
        for &name in NAMES {
            assert!(
                build_aggregator(name, AggregatorParams::default()).is_ok(),
                "{name} failed to build"
            );
        }
    }

    #[test]
    fn build_aggregator_fails_for_an_unknown_name() {
        // `Box<dyn Aggregator>` isn't `Debug`, so `.unwrap_err()` (which
        // needs the `Ok` side to be `Debug` for its panic message) isn't
        // usable here — match directly instead.
        match build_aggregator("does_not_exist", AggregatorParams::default()) {
            Err(AggregatorBuildError::Unknown(name)) => assert_eq!(name, "does_not_exist"),
            Ok(_) => panic!("expected an error, got a constructed Aggregator"),
        }
    }

    /// Catches `inventory::submit!` and `build_aggregator`'s match arms
    /// drifting apart as the family grows — every name one accepts must
    /// also be found by the other.
    #[test]
    fn every_buildable_name_is_also_registry_visible() {
        for &name in NAMES {
            assert!(build_aggregator(name, AggregatorParams::default()).is_ok());
            assert!(
                conflux_config::lookup(StrategyKind::Aggregator, name).is_some(),
                "{name} is buildable but not registry-visible"
            );
        }
    }
}
