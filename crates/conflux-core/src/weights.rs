//! This crate's shared, not-per-family-member logic: decoding a batch,
//! and the vectorized accumulation every family member's combine step
//! reduces to.
//!
//! Decoding `ClientDelta.weights` uses this crate's error type — the
//! actual little-endian `f32` codec lives in `conflux-proto` (shared
//! with `conflux-server`) so it's implemented once, not per
//! crate.

use conflux_proto::ClientDelta;

use crate::AggregatorError;

pub(crate) fn decode_weights(client_id: &str, bytes: &[u8]) -> Result<Vec<f32>, AggregatorError> {
    conflux_proto::decode_weights(bytes).map_err(|_| AggregatorError::MalformedWeights {
        client_id: client_id.to_string(),
        len: bytes.len(),
    })
}

/// The largest `num_samples` a client may report.
///
/// `2^40`, about 1.1 trillion. Chosen to be unmistakably beyond any real
/// federated client's local dataset while leaving several orders of
/// magnitude of headroom above the largest plausible one, so this can
/// never reject an honest participant.
///
/// **What this does and does not defend against.** It closes the
/// degenerate case: a client reporting `u64::MAX` gets `1.8e19` as an
/// `f32` weight, against an honest client's `10.0`, so every honest
/// contribution underflows to nothing and the liar's update *becomes*
/// the aggregate exactly. Measured before this check existed: one client
/// moved FedAvg's output from the honest consensus `[1.0, 1.0]` to its
/// own `[99.0, 99.0]`.
///
/// It does **not** stop a client that exaggerates within plausible
/// bounds — claiming 100,000 samples against everyone else's 10 still
/// buys 10,000x the influence, and no absolute ceiling can distinguish
/// that from a genuinely large participant. Two real defenses exist for
/// that, and neither is this constant:
///
/// 1. A robust aggregator. `krum` ignores `num_samples` entirely and
///    returned the honest consensus unharmed in the same measurement.
/// 2. Not accepting unauthenticated sample counts in the first place —
///    FedAvg's weighting assumes honest reporting (McMahan et al. 2017),
///    and that assumption is the framework's to surface, not to silently
///    repair by deviating from the published method.
pub const MAX_PLAUSIBLE_SAMPLE_COUNT: u64 = 1 << 40;

/// Decodes every update in a batch and checks it is fit to aggregate:
/// all the same length, all finite, and reporting a possible sample
/// count.
///
/// The "before any real aggregation logic can run" step every family
/// member needs (factored out here so `averaging.rs` and
/// `robust.rs`'s coordinate-wise members don't each reimplement it).
/// Being the single chokepoint is what makes it the right place for the
/// finiteness check — all eleven aggregator entry points call it, so no
/// method can forget.
///
/// Empty `updates` isn't checked here — callers decide whether an empty
/// batch is `EmptyBatch` or something else, since not every caller wants
/// the same error for it (this function just has nothing to validate
/// when there's nothing to decode).
///
/// **Public because an out-of-tree `Aggregator` needs it.** Anyone
/// implementing the trait outside this crate — a prototype, a method
/// this catalog does not ship — has to decode a batch and reject
/// non-finite weights before touching them, and reimplementing that is
/// how a new method acquires the `NaN`-handling defects the catalog
/// already fixed. Exporting the chokepoint is cheaper than watching it
/// be copied badly.
pub fn decode_and_validate(updates: &[ClientDelta]) -> Result<Vec<Vec<f32>>, AggregatorError> {
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

    for (update, weights) in updates.iter().zip(&decoded) {
        // Reported by index, not just as "this client is bad": a real
        // client hitting this is usually diverging during training, and
        // *where* in the parameter vector is what tells its operator
        // which layer blew up.
        if let Some(index) = weights.iter().position(|w| !w.is_finite()) {
            return Err(AggregatorError::NonFiniteWeights {
                client_id: update.client_id.clone(),
                index,
            });
        }
        if update.num_samples > MAX_PLAUSIBLE_SAMPLE_COUNT {
            return Err(AggregatorError::ImplausibleSampleCount {
                client_id: update.client_id.clone(),
                got: update.num_samples,
                max: MAX_PLAUSIBLE_SAMPLE_COUNT,
            });
        }
    }

    Ok(decoded)
}

// --- shared accumulation ----------------------------------
//
// Every shipped aggregation method's combine step reduces to one of two
// element-wise shapes. They live here, once, rather than as eight
// near-identical inline loops across `averaging.rs`, `robust.rs`, and
// `temporal.rs` — the same "common accumulation logic written once"
// principle ADR 0002's family pattern is built on.
//
// **These are deliberately scalar, and that is the measured result of
// this phase, not an omission.** set out to vectorize this loop
// with the `wide` crate. It was built, and then benchmarked against the
// scalar loop it replaced (`benches/accumulate.rs`, still present so the
// question stays answerable). Explicit `f32x8` SIMD was **slower** at
// every realistic model dimension:
//
//   dim        scalar      explicit SIMD
//   8          10.1 ns     3.6 ns     (SIMD ~2.8x faster)
//   10,000     1.21 us     1.23 us    (a wash)
//   1,000,000  145 us      154 us     (SIMD ~6% slower)
//
// The reason is that this loop is memory-bandwidth-bound at any size a
// real model has — at dim=1M it moves ~12 MB in ~145 us, which is
// already near memory bandwidth — and LLVM auto-vectorizes the plain
// scalar loop anyway. Rebuilding with `-C target-cpu=native` (AVX2 and
// AVX-512 both available on the test machine) made the *scalar* loop
// 2.5x faster at dim=8 and left the comparison at large dims unchanged,
// confirming both halves of that explanation. Hand-written SIMD only won
// at dim=8, which is not a size any model has.
//
// So the refactor stayed and the SIMD did not. If someone proposes
// vectorizing this again, `cargo bench -p conflux-core` is the answer.

/// `acc[i] += src[i] * weight`.
///
/// The workhorse: `WeightedAverageAggregator`'s combine, FoolsGold's,
/// and the plain summation paths (which pass `weight = 1.0`) all reduce
/// to this.
pub(crate) fn accumulate_weighted(acc: &mut [f32], src: &[f32], weight: f32) {
    // An internal invariant, not a user-facing error: every caller has
    // already been through `decode_and_validate`, which is what turns a
    // real length mismatch into `AggregatorError::MismatchedLength`.
    debug_assert_eq!(
        acc.len(),
        src.len(),
        "accumulate_weighted requires equal lengths; callers validate this upstream"
    );

    for (a, s) in acc.iter_mut().zip(src) {
        *a += s * weight;
    }
}

/// `acc[i] += (src[i] - reference[i]) * scale`, accumulated in `f64`.
///
/// Centered Clipping's combine step — the same element-wise shape as
/// [`accumulate_weighted`] with one extra subtraction, and the only
/// combine step in the crate that isn't a plain weighted sum.
///
/// The accumulator is `f64` while the inputs stay `f32`, and that
/// asymmetry is the point. `src[i] - reference[i]` overflows `f32` to
/// infinity whenever a client sits far from the reference — two finite
/// `f32` weights can be `2 · f32::MAX` apart — and the caller's own
/// clip scale is near zero in exactly that case, because a distant
/// client is what clipping exists to damp. `inf * 0.0` is `NaN`, so the
/// step that correctly decided "this client moves the model by nothing"
/// wrote `NaN` into the running reference instead, and every later
/// round clipped against it. Subtracting in `f64` cannot overflow for
/// any finite `f32` pair, which removes the infinity the `NaN` was made
/// from.
pub(crate) fn accumulate_scaled_difference(
    acc: &mut [f64],
    src: &[f32],
    reference: &[f32],
    scale: f64,
) {
    debug_assert_eq!(acc.len(), src.len());
    debug_assert_eq!(acc.len(), reference.len());

    for (a, (s, r)) in acc.iter_mut().zip(src.iter().zip(reference)) {
        *a += (*s as f64 - *r as f64) * scale;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The inline loops these helpers replaced, kept verbatim as the
    /// reference each helper must still match exactly. The refactor's
    /// whole correctness bar is that no aggregator's output changed, so
    /// these compare bit patterns rather than values.
    fn scalar_accumulate_weighted(acc: &mut [f32], src: &[f32], weight: f32) {
        for (a, s) in acc.iter_mut().zip(src) {
            *a += s * weight;
        }
    }

    fn scalar_accumulate_scaled_difference(
        acc: &mut [f64],
        src: &[f32],
        reference: &[f32],
        scale: f64,
    ) {
        for (a, (s, r)) in acc.iter_mut().zip(src.iter().zip(reference)) {
            *a += (*s as f64 - *r as f64) * scale;
        }
    }

    /// Deterministic, spread across magnitudes so rounding actually has
    /// something to disagree about — a vector of small similar values
    /// would pass a bit-identity test that a genuinely wrong
    /// implementation could also pass.
    fn sample(len: usize, offset: f32) -> Vec<f32> {
        (0..len)
            .map(|i| (i as f32 * 0.37 + offset) * if i % 3 == 0 { -1.234e3 } else { 5.678e-4 })
            .collect()
    }

    #[test]
    fn accumulate_weighted_is_bit_identical_to_the_loop_it_replaced() {
        // Lengths spanning the 8-element boundary the SIMD version
        // chunked at, kept so this test still covers those shapes if
        // anyone reintroduces a chunked implementation.
        for len in [0, 1, 2, 7, 8, 9, 15, 16, 17, 31, 100, 1000, 1023] {
            for weight in [0.0, 1.0, -1.0, 0.25, 1e-7, 3.7e5] {
                let src = sample(len, 1.0);
                let start = sample(len, -2.0);

                let mut helper = start.clone();
                accumulate_weighted(&mut helper, &src, weight);

                let mut reference_out = start;
                scalar_accumulate_weighted(&mut reference_out, &src, weight);

                // `to_bits` rather than `==`: this asserts bit
                // identity, which is the actual claim. `==` would treat
                // +0.0 and -0.0 as equal and say nothing about NaN.
                let helper_bits: Vec<u32> = helper.iter().map(|f| f.to_bits()).collect();
                let reference_bits: Vec<u32> = reference_out.iter().map(|f| f.to_bits()).collect();
                assert_eq!(
                    helper_bits, reference_bits,
                    "len={len} weight={weight}: the helper and the original loop disagree"
                );
            }
        }
    }

    #[test]
    fn accumulate_scaled_difference_is_bit_identical_to_the_loop_it_replaced() {
        for len in [0, 1, 7, 8, 9, 17, 100, 1000] {
            for scale in [0.0, 1.0, 0.125, -3.5] {
                let src = sample(len, 1.0);
                let reference = sample(len, 0.5);
                let start: Vec<f64> = sample(len, -2.0).iter().map(|x| *x as f64).collect();

                let mut helper = start.clone();
                accumulate_scaled_difference(&mut helper, &src, &reference, scale);

                let mut reference_out = start;
                scalar_accumulate_scaled_difference(&mut reference_out, &src, &reference, scale);

                let helper_bits: Vec<u64> = helper.iter().map(|f| f.to_bits()).collect();
                let reference_bits: Vec<u64> = reference_out.iter().map(|f| f.to_bits()).collect();
                assert_eq!(helper_bits, reference_bits, "len={len} scale={scale}");
            }
        }
    }

    #[test]
    fn accumulate_weighted_handles_a_zero_length_input() {
        let mut acc: Vec<f32> = Vec::new();
        accumulate_weighted(&mut acc, &[], 1.0);
        assert!(acc.is_empty());
    }
}
