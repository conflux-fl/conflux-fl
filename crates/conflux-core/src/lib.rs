//! SIMD aggregation algorithms (family-based).
//!
//! See `docs/spec/conflux-spec-v1.md` §5.

mod averaging;
mod robust;
mod weights;

pub use averaging::{AveragingWeighting, FedAvg, SampleCountWeighting, WeightedAverageAggregator};
pub use robust::{
    CoordinateWiseAggregator, CoordinateWiseRobustStatistic, DistanceMatrix, FilteredAggregator,
    KrumFilter, MedianStatistic, MultiKrumFilter, SelectionResult, TrimmedMeanStatistic,
    UpdateFilter,
};

use conflux_config::{StrategyEntry, StrategyKind};
use conflux_proto::ClientDelta;

/// Turns one round's batch of client updates into new global weights.
/// Spec §5 — every aggregation family member implements this: `FedAvg`
/// directly, and `Krum`/`Multi-Krum`/`Trimmed Mean`/`Median` (Phase 11a)
/// via `FilteredAggregator`/`CoordinateWiseAggregator`.
pub trait Aggregator: Send + Sync {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError>;
}

// Phase 10b/11a: registers every shipped family member into
// `conflux-config`'s compile-time strategy registry (ADR 0002) —
// `build_aggregator` is what actually turns a config-resolved name into
// a constructed `Aggregator`; these submissions are what let
// `conflux_config::lookup` find each name at all, independent of whether
// anything ever calls `build_aggregator`.
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

#[derive(Debug, thiserror::Error)]
pub enum AggregatorBuildError {
    #[error(
        "unknown aggregator \"{0}\" — not a registered conflux-core strategy \
         (known: \"fedavg\", \"krum\", \"multi_krum\", \"trimmed_mean\", \"median\")"
    )]
    Unknown(String),
}

/// Constructs the `Aggregator` named by a resolved `config.aggregator.value`.
/// Every match arm mirrors one `inventory::submit!` above — adding a
/// family member means adding both, not restructuring this function.
/// `byzantine_fraction` (`config.robust_byzantine_fraction.value`, Phase
/// 11a) is only read by the `robust` family's members; `fedavg` ignores
/// it entirely.
pub fn build_aggregator(
    name: &str,
    byzantine_fraction: f32,
) -> Result<Box<dyn Aggregator>, AggregatorBuildError> {
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
        other => Err(AggregatorBuildError::Unknown(other.to_string())),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AggregatorError {
    #[error("cannot aggregate an empty batch of updates")]
    EmptyBatch,
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

    const NAMES: &[&str] = &["fedavg", "krum", "multi_krum", "trimmed_mean", "median"];

    #[test]
    fn build_aggregator_succeeds_for_every_shipped_name() {
        for &name in NAMES {
            assert!(
                build_aggregator(name, 0.2).is_ok(),
                "{name} failed to build"
            );
        }
    }

    #[test]
    fn build_aggregator_fails_for_an_unknown_name() {
        // `Box<dyn Aggregator>` isn't `Debug`, so `.unwrap_err()` (which
        // needs the `Ok` side to be `Debug` for its panic message) isn't
        // usable here — match directly instead.
        match build_aggregator("does_not_exist", 0.2) {
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
            assert!(build_aggregator(name, 0.2).is_ok());
            assert!(
                conflux_config::lookup(StrategyKind::Aggregator, name).is_some(),
                "{name} is buildable but not registry-visible"
            );
        }
    }
}
