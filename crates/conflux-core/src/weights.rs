//! Decodes `ClientDelta.weights` for this crate's error type — the actual
//! little-endian `f32` codec lives in `conflux-proto` (shared with
//! `conflux-server`, Phase 5) so it's implemented once, not per crate.

use conflux_proto::ClientDelta;

use crate::AggregatorError;

pub(crate) fn decode_weights(client_id: &str, bytes: &[u8]) -> Result<Vec<f32>, AggregatorError> {
    conflux_proto::decode_weights(bytes).map_err(|_| AggregatorError::MalformedWeights {
        client_id: client_id.to_string(),
        len: bytes.len(),
    })
}

/// Decodes every update in a batch and checks they're all the same
/// length — the "before any real aggregation logic can run" step every
/// aggregator family member needs (Phase 11a: factored out here so
/// `averaging.rs` and `robust.rs`'s coordinate-wise members don't each
/// reimplement it). Empty `updates` isn't checked here — callers decide
/// whether an empty batch is `EmptyBatch` or something else, since not
/// every caller wants the same error for it (this function just has
/// nothing to validate when there's nothing to decode).
pub(crate) fn decode_and_validate(
    updates: &[ClientDelta],
) -> Result<Vec<Vec<f32>>, AggregatorError> {
    let decoded: Vec<Vec<f32>> = updates
        .iter()
        .map(|u| decode_weights(&u.client_id, &u.weights))
        .collect::<Result<_, _>>()?;

    if let Some(first) = decoded.first() {
        let dim = first.len();
        for (update, weights) in updates.iter().zip(&decoded) {
            if weights.len() != dim {
                return Err(AggregatorError::MismatchedLength {
                    client_id: update.client_id.clone(),
                    expected: dim,
                    got: weights.len(),
                });
            }
        }
    }

    Ok(decoded)
}
