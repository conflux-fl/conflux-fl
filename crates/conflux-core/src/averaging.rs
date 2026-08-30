//! The `averaging` aggregation family: shared weighted-mean accumulation,
//! written once, plus a small trait capturing what each member varies.
//! `FedAvg` is the one member currently shipped.

use conflux_proto::ClientDelta;

use crate::weights::{accumulate_weighted, decode_and_validate};
use crate::{Aggregator, AggregatorError};

/// What a member of the `averaging` family varies: how much weight one
/// update gets relative to the rest of the batch. `WeightedAverageAggregator`
/// normalizes whatever this returns to sum to 1 across the batch, so an
/// implementation only needs to return a relative weight, not a
/// pre-normalized fraction.
pub trait AveragingWeighting: Send + Sync {
    fn weight_for(&self, update: &ClientDelta, batch: &[ClientDelta]) -> f32;
}

/// Shared accumulation logic for the whole `averaging` family: decode
/// each update, ask `W` how much it counts, normalize, accumulate. A new
/// family member — e.g. inverse-loss weighting — is a new
/// `AveragingWeighting` impl, not a new `Aggregator`.
pub struct WeightedAverageAggregator<W: AveragingWeighting> {
    weighting: W,
}

impl<W: AveragingWeighting> WeightedAverageAggregator<W> {
    pub fn new(weighting: W) -> Self {
        Self { weighting }
    }
}

impl<W: AveragingWeighting> Aggregator for WeightedAverageAggregator<W> {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }

        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let raw_weights: Vec<f32> = updates
            .iter()
            .map(|u| self.weighting.weight_for(u, updates))
            .collect();
        let total: f32 = raw_weights.iter().sum();
        if total == 0.0 {
            return Err(AggregatorError::ZeroWeightSum);
        }

        let mut accumulator = vec![0.0f32; dim];
        for (raw_weight, weights) in raw_weights.iter().zip(&decoded) {
            let normalized = raw_weight / total;
            accumulate_weighted(&mut accumulator, weights, normalized);
        }

        Ok(accumulator)
    }
}

/// FedAvg's weighting rule (McMahan et al., 2017): weight each update by
/// its sample count. `WeightedAverageAggregator::aggregate` does the
/// normalization (`n_k / Σn_i`), so this impl only returns the raw count —
/// a family member is typically about this small, a short trait impl
/// reusing the shared accumulation logic above.
#[derive(Default)]
pub struct SampleCountWeighting;

impl AveragingWeighting for SampleCountWeighting {
    fn weight_for(&self, update: &ClientDelta, _batch: &[ClientDelta]) -> f32 {
        update.num_samples as f32
    }
}

/// FedAvg (McMahan et al., 2017) as a concrete type: `WeightedAverageAggregator`
/// specialized with `SampleCountWeighting`.
pub type FedAvg = WeightedAverageAggregator<SampleCountWeighting>;

// `FedAvg` is a type alias, not a distinct type, so it can't carry its own
// inherent `new()` alongside `WeightedAverageAggregator::new`'s generic
// one without a name clash — `Default` (built generically below) is what
// gives `FedAvg::default()` a zero-argument constructor instead.
impl<W: AveragingWeighting + Default> Default for WeightedAverageAggregator<W> {
    fn default() -> Self {
        Self::new(W::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client_delta(client_id: &str, weights: &[f32], num_samples: u64) -> ClientDelta {
        let mut bytes = Vec::with_capacity(weights.len() * 4);
        for w in weights {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        ClientDelta {
            client_id: client_id.to_string(),
            round: 1,
            weights: bytes,
            num_samples,
        }
    }

    #[test]
    fn equal_sample_counts_average_to_the_elementwise_mean() {
        let fed_avg = FedAvg::default();
        let updates = vec![
            client_delta("a", &[1.0, 2.0], 10),
            client_delta("b", &[3.0, 4.0], 10),
        ];

        let result = fed_avg.aggregate(&updates).unwrap();

        assert_eq!(result, vec![2.0, 3.0]);
    }

    #[test]
    fn larger_sample_count_pulls_the_result_toward_it() {
        let fed_avg = FedAvg::default();
        let updates = vec![
            client_delta("small", &[0.0], 1),
            client_delta("large", &[10.0], 99),
        ];

        let result = fed_avg.aggregate(&updates).unwrap();

        // Plain mean would be 5.0; the heavily-sampled update should pull
        // the weighted result much closer to 10.0.
        assert!(result[0] > 9.0);
    }

    #[test]
    fn single_update_batch_is_unchanged() {
        let fed_avg = FedAvg::default();
        let updates = vec![client_delta("only", &[1.0, -2.0, 3.0], 5)];

        let result = fed_avg.aggregate(&updates).unwrap();

        assert_eq!(result, vec![1.0, -2.0, 3.0]);
    }

    #[test]
    fn empty_batch_errors() {
        let fed_avg = FedAvg::default();

        let err = fed_avg.aggregate(&[]).unwrap_err();

        assert!(matches!(err, AggregatorError::EmptyBatch));
    }

    #[test]
    fn mismatched_lengths_error() {
        let fed_avg = FedAvg::default();
        let updates = vec![
            client_delta("a", &[1.0, 2.0], 10),
            client_delta("b", &[3.0], 10),
        ];

        let err = fed_avg.aggregate(&updates).unwrap_err();

        assert!(matches!(err, AggregatorError::MismatchedLength { .. }));
    }
}
