//! Cross-round, history-aware aggregation — methods that need memory
//! across rounds, unlike every member of `averaging`/`robust`, which
//! judges each round's batch in isolation.
//!
//! Why the family exists: no stateless method can distinguish a
//! colluding Sybil cluster from a legitimate majority within a single
//! round, because both can produce the same single-round batch geometry
//! by construction. Telling them apart needs history.

use std::collections::HashMap;
use std::sync::Mutex;

use conflux_proto::ClientDelta;

use crate::weights::{accumulate_scaled_difference, accumulate_weighted, decode_and_validate};
use crate::{Aggregator, AggregatorError};

/// L2 distance, accumulated in `f64`.
///
/// The `f64` is not precision fussiness — it is the difference between
/// a number and `NaN`. Two *finite* `f32` weights can be up to
/// `2 · f32::MAX` apart, and squaring that overflows `f32` to infinity
/// long before any input is unreasonable. An infinite distance then
/// poisons everything downstream of it: a variance becomes `inf - inf`
/// (`NaN`), and a clip scale becomes `τ / inf` (`0`), which multiplied
/// back against the infinite deviation is `NaN` again.
///
/// `f64` has the range to hold every one of these intermediates
/// exactly — the largest possible squared difference is about `4.6e77`,
/// against `f64`'s `1.8e308` ceiling — so nothing here can overflow for
/// any finite `f32` input. Callers keep the `f64` for arithmetic and
/// narrow only a final, bounded result.
fn l2_distance_f64(a: &[f32], b: &[f32]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = *x as f64 - *y as f64;
            d * d
        })
        .sum::<f64>()
        .sqrt()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// The FoolsGold scoring function itself, translated directly from the
/// authors' own reference implementation
/// (`deep-fg/fg/foolsgold.py::foolsgold`,
/// <https://github.com/DistributedML/FoolsGold>) rather than from the
/// paper's prose description alone — the prose leaves the "pardoning"
/// step's exact loop structure and the logit-clamping edge cases
/// underspecified enough that a from-scratch reading can get the
/// pardoning direction backwards. Kept as its own
/// function, separate from `Aggregator::aggregate`, so it can be unit
/// tested directly against hand-computed values without going through
/// weight decoding.
///
/// `histories[i]` is client `i`'s cumulative historical update.
/// Returns one weight per client in `[0, 1]`.
fn foolsgold_weights(histories: &[Vec<f32>]) -> Vec<f32> {
    let n = histories.len();

    // cs[i][j] = cosine_similarity(history_i, history_j), diagonal left
    // at 0 (matches the reference's `cosine_similarity(grads) - eye(n)`).
    let mut cs = vec![vec![0.0f32; n]; n];
    for i in 0..n {
        for j in 0..n {
            if i != j {
                cs[i][j] = cosine_similarity(&histories[i], &histories[j]);
            }
        }
    }

    // maxcs[i] = max over the whole row, including the zeroed diagonal —
    // so a client with only negative similarities to everyone else still
    // floors at 0, exactly matching `np.max(cs, axis=1)` over a
    // diagonal-zeroed row.
    let row_max = |cs: &[Vec<f32>], i: usize| cs[i].iter().cloned().fold(0.0f32, f32::max);
    let mut maxcs: Vec<f32> = (0..n).map(|i| row_max(&cs, i)).collect();

    // Pardoning: reference loops over *every* (i, j) pair, not just each
    // row's argmax — for every j whose own max similarity exceeds i's,
    // i's specific similarity to j is scaled down by maxcs[i]/maxcs[j].
    // `maxcs[j]` is never 0 when this branch runs, since maxcs is always
    // >= 0 (the diagonal floor) and the guard already requires
    // `maxcs[i] < maxcs[j]`, which is impossible if maxcs[j] == 0.
    for i in 0..n {
        for j in 0..n {
            if i != j && maxcs[i] < maxcs[j] {
                cs[i][j] *= maxcs[i] / maxcs[j];
            }
        }
    }
    maxcs = (0..n).map(|i| row_max(&cs, i)).collect();

    // wv = 1 - maxcs, clipped to [0, 1].
    let mut wv: Vec<f32> = maxcs.iter().map(|&m| (1.0 - m).clamp(0.0, 1.0)).collect();

    // Rescale so the least-suspicious client's wv is 1 — unless every
    // client is maximally suspicious of everyone (max_wv == 0), a
    // degenerate case the reference implementation itself doesn't handle
    // cleanly (produces NaN via 0/0); return all-zero weights here
    // instead, which `aggregate` below falls back on sensibly.
    let max_wv = wv.iter().cloned().fold(0.0f32, f32::max);
    if max_wv <= 0.0 {
        return vec![0.0; n];
    }
    for w in &mut wv {
        *w /= max_wv;
    }
    // The client(s) at exactly 1.0 after rescaling would make the logit
    // step below divide by zero (`ln(1.0 / 0.0)`) — cap to 0.99 first,
    // exactly matching the reference's `wv[(wv == 1)] = .99`.
    for w in &mut wv {
        if *w >= 1.0 {
            *w = 0.99;
        }
    }

    // Logistic sharpening. `logit > 1.0` is `true` for `+inf` and any
    // finite logit past 1 alike; `logit < 0.0` likewise covers `-inf` —
    // `f32::clamp` handles both without a separate `is_infinite` check,
    // matching the reference's two-line `isinf`/`< 0` clamp exactly.
    for w in &mut wv {
        let logit = (*w / (1.0 - *w)).ln() + 0.5;
        *w = logit.clamp(0.0, 1.0);
    }

    wv
}

/// FoolsGold (Fung, Yoon & Beznosov, 2018/2020, *The Limitations of
/// Federated Learning in Sybil Settings*, RAID 2020): maintains each
/// client's cumulative historical update (summed across every round it
/// has participated in) and scores clients by pairwise cosine similarity
/// of their *histories* — the signature of Sybils reinforcing each other
/// round after round, invisible to every stateless method in this crate,
/// since a single round's identical-looking colluding updates are
/// diluted by whatever that client's history looked like before.
/// Clients whose history is suspiciously similar to another's are
/// down-weighted; clients with a unique historical pattern keep full
/// weight.
///
/// **Why this needs its own module, not `robust.rs`**: `Aggregator::
/// aggregate` takes `&self`; state that must survive across calls needs
/// interior mutability (`Mutex`), a genuinely different shape from every
/// stateless family member `robust.rs`/`averaging.rs` define. History is
/// in-memory only for now — surviving a server restart is the same class
/// of problem solved for the privacy accountant, not solved
/// here, and a real follow-up before this is relied on across restarts.
///
/// **Fidelity note**: `foolsgold_weights` above is a direct
/// translation of the authors' reference implementation
/// (`deep-fg/fg/foolsgold.py`), not a from-scratch reading of the paper —
/// verified line-by-line, including the pardoning loop's exact structure
/// and the logit step's edge-case clamping. The **combine step**
/// (weighted sum divided by client count `n`, not by the sum of weights
/// or by `num_samples`) also matches the reference's
/// `deep-fg/fg/trainer.py::aggregate_gradients` exactly, deliberately —
/// this is the one aggregator in this crate that does *not* follow the
/// rest of the codebase's num_samples-weighting convention, so that
/// results are directly comparable against the original paper's own
/// experimental setup rather than a modified variant of it.
pub struct FoolsGoldAggregator {
    history: Mutex<HashMap<String, Vec<f32>>>,
}

impl Default for FoolsGoldAggregator {
    fn default() -> Self {
        Self {
            history: Mutex::new(HashMap::new()),
        }
    }
}

impl Aggregator for FoolsGoldAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let n = updates.len();

        let histories: Vec<Vec<f32>> = {
            let mut history = self
                .history
                .lock()
                .expect("FoolsGoldAggregator history mutex poisoned");
            updates
                .iter()
                .zip(&decoded)
                .map(|(u, w)| {
                    let entry = history
                        .entry(u.client_id.clone())
                        .or_insert_with(|| vec![0.0f32; w.len()]);
                    for (h, x) in entry.iter_mut().zip(w) {
                        *h += x;
                    }
                    entry.clone()
                })
                .collect()
        };

        if n == 1 {
            return Ok(decoded[0].clone());
        }

        let weights = foolsgold_weights(&histories);

        let dim = decoded[0].len();
        let mut combined = vec![0.0f32; dim];
        let all_zero = weights.iter().all(|&w| w == 0.0);
        // The `1/n` both branches used to apply afterwards is folded into
        // each term instead: `accumulate_weighted` multiplies before it
        // adds, so scaling here keeps the running total inside the range
        // of the values feeding it. Dividing a total that has already
        // overflowed to infinity does nothing.
        let share = 1.0 / n as f32;
        if all_zero {
            // Degenerate case `foolsgold_weights` itself falls back on
            // (see its own doc comment) — an unweighted mean rather than
            // producing an all-zero aggregate.
            for w in &decoded {
                accumulate_weighted(&mut combined, w, share);
            }
        } else {
            for (i, w) in weights.iter().enumerate() {
                accumulate_weighted(&mut combined, &decoded[i], *w * share);
            }
        }

        Ok(combined)
    }
}

/// **Centered Clipping** — Karimireddy, He & Jaggi, 2021, "Learning from
/// History for Byzantine Robust Optimization" (ICML), Algorithm 1.
///
/// The insight the paper is built on: every single-round robust method
/// (Krum, Trimmed Mean, Median, ...) throws away the one signal that
/// makes a persistent attacker detectable — what the model looked like
/// last round. Centered Clipping keeps a running reference vector `v`
/// across rounds and clips each client's *deviation from that reference*
/// to a radius `τ`, rather than clipping raw updates against a fixed
/// origin or discarding suspicious clients outright.
///
/// That distinction is the whole method, and it is easy to get wrong:
/// clipping `u_i` itself to norm `τ` would shrink every client toward
/// the origin (meaningless for full weight vectors); clipping
/// `u_i − v` bounds only how far any one client can *move the model*,
/// which is exactly the quantity an attacker needs unbounded to do
/// damage. Nobody is excluded — a Byzantine client still contributes,
/// just never more than `τ`-worth of pull. That is why the method
/// degrades gracefully rather than falling apart when the assumed
/// attacker count is wrong, unlike the selection-based `robust` members.
///
/// Per round, with `n` updates `u_i`:
///
/// ```text
/// v ← v + (1/n) Σ_i  min(1, τ / ‖u_i − v‖) · (u_i − v)
/// ```
///
/// and that new `v` is both the round's aggregate and the next round's
/// reference — the cross-round state that puts this in `temporal` rather
/// than `robust`, despite being a Byzantine-robustness method.
///
/// **Fidelity notes:**
/// - The combine step is an unweighted `1/n` mean of clipped deviations,
///   matching the paper. Like [`FoolsGoldAggregator`], this deliberately
///   does *not* follow the codebase's `num_samples`-weighting convention
///   — results stay directly comparable to the published experiments.
/// - The paper initializes `v` to the zero vector (or a warm start). A
///   zero start assumes updates are gradient-like and small; Conflux
///   transmits **full model weights**, where clipping every client's
///   deviation from the origin would gut round one. So `v` starts as
///   `None` and is seeded from the first round's plain mean — one of the
///   paper's own permitted warm starts, and the only one whose scale is
///   knowable before a batch has been seen. The recursion itself is
///   unmodified. The cost is real and worth stating: round one centers
///   on a mean an attacker can drag, so the defense compounds over
///   rounds rather than arriving fully formed in round one.
/// - `τ` is problem-scale dependent — the paper tunes it per experiment,
///   and so must a deployment. It is config-resolved (`clip_radius`),
///   never a hardcoded constant. **In the MNIST harness (a
///   50,890-parameter MLP), the placeholder `τ = 1.0` scored *worse than
///   no defense*, and no `τ` between 1 and 100 reached a selection-based
///   method's accuracy.** `τ` bounds an L2
///   norm in parameter space, so what it buys per round depends on how
///   many parameters that norm is spread across; an optimum found at
///   `dim = 3` does not transfer to `dim = 50,890`. Treat an untuned
///   `clip_radius` as an unconfigured deployment. A negative `τ` would invert the
///   clipping rather than bound it; like the `robust` family's
///   `byzantine_fraction`, it is documented rather than validated, since
///   the config layer is where an operator-supplied value belongs.
pub struct CenteredClippingAggregator {
    clip_radius: f32,
    /// `None` until the first batch has been seen — see the fidelity
    /// note on initialization. `Mutex` for the same reason
    /// [`FoolsGoldAggregator`]'s history uses one: `Aggregator::aggregate`
    /// takes `&self` (so one aggregator can serve concurrent rounds
    /// behind an `Arc`), and interior mutability is what lets a method
    /// carry state across rounds without changing that shared signature.
    reference: Mutex<Option<Vec<f32>>>,
}

impl CenteredClippingAggregator {
    /// An aggregator clipping each client's deviation from the running
    /// reference to `clip_radius`.
    ///
    /// `clip_radius` must be tuned to the model's weight scale — see this
    /// type's fidelity notes for what the untuned default measured.
    pub fn new(clip_radius: f32) -> Self {
        Self {
            clip_radius,
            reference: Mutex::new(None),
        }
    }

    /// The current reference vector, or `None` before the first round.
    /// Read-only — exposed for tests and diagnostics, never consulted by
    /// `aggregate` itself.
    pub fn reference(&self) -> Option<Vec<f32>> {
        self.reference
            .lock()
            .expect("CenteredClippingAggregator reference mutex poisoned")
            .clone()
    }
}

impl Aggregator for CenteredClippingAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let n = decoded.len();
        let dim = decoded[0].len();

        let mut reference = self
            .reference
            .lock()
            .expect("CenteredClippingAggregator reference mutex poisoned");

        // Re-seeding on a dimension change rather than erroring: a model
        // whose weight-vector length changed mid-experiment has no
        // meaningful continuity with the old reference, so carrying it
        // forward would be worse than starting over.
        let v: Vec<f32> = match reference.as_ref() {
            Some(prev) if prev.len() == dim => prev.clone(),
            _ => {
                // Scale each update by `1/n` *before* adding it, not
                // after summing. Summing first overflows to infinity
                // once the batch total exceeds `f32::MAX` — four clients
                // near `f32::MAX` is enough — and `inf / n` is still
                // infinity, so the reference starts the experiment
                // already destroyed. The same discipline `robust.rs`'s
                // geometric median follows.
                let mut mean = vec![0.0f32; dim];
                let share = 1.0 / n as f32;
                for w in &decoded {
                    accumulate_weighted(&mut mean, w, share);
                }
                mean
            }
        };

        // Accumulated in `f64`, and this is load-bearing rather than
        // tidy. In `f32` the deviation `u_i − v` overflows to infinity
        // whenever a client is far from the reference, which makes
        // `distance` infinite, which makes `scale` exactly `0.0` — and
        // `inf * 0.0` is `NaN`. So the clipping step decided correctly
        // that this client should move the model by nothing, and then
        // wrote `NaN` into the stored reference, permanently: every
        // later round clips against `NaN`, every aggregate is `NaN`, and
        // no subsequent honest round can recover it. A finite,
        // validation-passing update was enough to trigger it.
        let mut clipped_sum = vec![0.0f64; dim];
        for w in &decoded {
            let distance = l2_distance_f64(w, &v);
            // `min(1, τ/d)`, with the `d == 0` case handled separately
            // because `τ/0` is a division by zero — though the deviation
            // it would scale is the zero vector either way, so the value
            // chosen there cannot affect the result.
            let scale = if distance > 0.0 {
                (self.clip_radius as f64 / distance).min(1.0)
            } else {
                1.0
            };
            accumulate_scaled_difference(&mut clipped_sum, w, &v, scale);
        }

        // Each client contributes at most `τ`-worth of pull, so this sum
        // is bounded by `n · τ` and `v + sum/n` stays within `τ` of a
        // finite `v` — the narrowing back to `f32` cannot overflow.
        let next: Vec<f32> = v
            .iter()
            .zip(&clipped_sum)
            .map(|(vi, acc)| (*vi as f64 + acc / n as f64) as f32)
            .collect();

        *reference = Some(next.clone());
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- foolsgold_weights, directly against the reference algorithm's
    // own arithmetic, independent of the aggregator/decoding plumbing ---

    #[test]
    fn two_orthogonal_histories_get_equal_weight() {
        // cosine(e0, e1) = 0 either direction -> maxcs = 0 for both ->
        // wv = 1 - 0 = 1 for both -> rescale (max already 1) -> both hit
        // the 0.99 cap -> identical logit -> identical final weight.
        let weights = foolsgold_weights(&[vec![1.0, 0.0], vec![0.0, 1.0]]);

        assert!((weights[0] - weights[1]).abs() < 1e-6, "got {weights:?}");
        assert!(weights[0] > 0.0, "got {weights:?}");
    }

    #[test]
    fn identical_histories_collapse_to_zero_weight() {
        // cosine(a, b) = 1 exactly -> maxcs = 1 for both -> wv = 0 for
        // both -> max_wv = 0 -> the degenerate all-zero fallback.
        let weights = foolsgold_weights(&[vec![5.0, 5.0], vec![5.0, 5.0]]);

        assert_eq!(weights, vec![0.0, 0.0]);
    }

    #[test]
    fn a_pair_colluding_among_mutually_orthogonal_honest_clients_is_down_weighted() {
        // Three mutually orthogonal honest histories (pairwise cosine 0,
        // so no residual suspicion between any two of them) plus two
        // identical colluders — a clean, hand-verifiable case: every
        // honest client's only nonzero similarity is to the colluders
        // (cosine 5/sqrt(75) ≈ 0.577 each), which pardoning reduces to
        // ≈0.577² ≈ 0.333 (since all three honest clients share the same
        // maxcs going into that step), giving all three the same,
        // maximal final weight (1.0, after the 0.99-cap and logit
        // sharpening), while the colluders' identical histories give
        // them cosine similarity 1.0 to each other and collapse to 0.
        let weights = foolsgold_weights(&[
            vec![1.0, 0.0, 0.0], // honest-1
            vec![0.0, 1.0, 0.0], // honest-2
            vec![0.0, 0.0, 1.0], // honest-3
            vec![5.0, 5.0, 5.0], // sybil-1
            vec![5.0, 5.0, 5.0], // sybil-2 (identical to sybil-1)
        ]);

        for &w in &weights[..3] {
            assert!(
                (w - 1.0).abs() < 1e-3,
                "expected honest clients at weight 1.0: {weights:?}"
            );
        }
        for &w in &weights[3..] {
            assert!(w < 1e-3, "expected colluders at weight 0.0: {weights:?}");
        }
    }

    // --- Aggregator wiring ---

    #[test]
    fn single_client_returns_it_unchanged() {
        let aggregator = FoolsGoldAggregator::default();
        let updates = vec![client_delta("only", &[3.0, -1.0])];

        let result = aggregator.aggregate(&updates).unwrap();

        assert_eq!(result, vec![3.0, -1.0]);
    }

    #[test]
    fn empty_batch_errors() {
        let aggregator = FoolsGoldAggregator::default();

        let err = aggregator.aggregate(&[]).unwrap_err();

        assert!(matches!(err, AggregatorError::EmptyBatch));
    }

    #[test]
    fn detects_and_down_weights_a_colluding_pair_across_rounds() {
        let aggregator = FoolsGoldAggregator::default();
        // Same configuration as `a_pair_colluding_among_mutually_
        // orthogonal_honest_clients_is_down_weighted`, run through the
        // full `Aggregator` (history accumulation + combine step), and
        // repeated across two rounds to prove it works under realistic
        // repeated calling, not just a single call.
        let round = || {
            vec![
                client_delta("honest-1", &[1.0, 0.0, 0.0]),
                client_delta("honest-2", &[0.0, 1.0, 0.0]),
                client_delta("honest-3", &[0.0, 0.0, 1.0]),
                client_delta("sybil-1", &[5.0, 5.0, 5.0]),
                client_delta("sybil-2", &[5.0, 5.0, 5.0]),
            ]
        };

        aggregator.aggregate(&round()).unwrap();
        let result = aggregator.aggregate(&round()).unwrap();

        // Honest clients converge to weight 1.0, sybils to 0.0 (see the
        // hand-derivation on the sibling test above); the combine step
        // divides by n = 5 (matching the reference exactly, not the sum
        // of weights — this module's own doc comment), so the expected
        // result is ([1,0,0]+[0,1,0]+[0,0,1]+0+0)/5 = [0.2, 0.2, 0.2].
        assert!(
            result.iter().all(|&x| (x - 0.2).abs() < 0.02),
            "got {result:?}, expected close to [0.2, 0.2, 0.2]"
        );
    }

    #[test]
    fn dissimilar_honest_clients_all_contribute() {
        let aggregator = FoolsGoldAggregator::default();
        let updates = vec![
            client_delta("a", &[1.0, 0.0]),
            client_delta("b", &[0.0, 1.0]),
            client_delta("c", &[-1.0, -1.0]),
        ];

        let result = aggregator.aggregate(&updates).unwrap();

        // No collusion signal -> every client keeps meaningful weight ->
        // result should resemble the plain average of all three, [0, 0],
        // not collapse toward any single client.
        assert!(
            result[0].abs() < 0.5 && result[1].abs() < 0.5,
            "got {result:?}"
        );
    }

    // --- Centered Clipping (Karimireddy, He & Jaggi 2021) -------------
    //
    // Every expectation below is derived by hand from the published
    // recursion, not read back off an implementation run.

    #[test]
    fn centered_clipping_bounds_each_clients_pull_to_exactly_the_clip_radius() {
        // tau = 1, batch {[0,0], [2,0], [10,0]} on a fresh aggregator.
        //   v_0   = mean = [4, 0]
        //   devs  = [-4,0] (d=4), [-2,0] (d=2), [6,0] (d=6)
        //   every d > tau, so each scales to exactly unit length:
        //         [-1,0],  [-1,0],  [1,0]
        //   sum   = [-1, 0];  /3 = [-1/3, 0]
        //   v_1   = [4,0] + [-1/3,0] = [11/3, 0]
        let agg = CenteredClippingAggregator::new(1.0);
        let out = agg
            .aggregate(&[
                client_delta("a", &[0.0, 0.0]),
                client_delta("b", &[2.0, 0.0]),
                client_delta("c", &[10.0, 0.0]),
            ])
            .unwrap();

        assert!((out[0] - 11.0 / 3.0).abs() < 1e-5, "got {out:?}");
        assert!(out[1].abs() < 1e-6, "got {out:?}");

        // The defining property: `c` sits 6 away from the reference but
        // moved it by no more than tau/n — the same bound as the client
        // sitting 2 away. Distance past the radius buys an attacker
        // nothing.
        let pull_of_c = 1.0 / 3.0;
        assert!(
            (4.0 - out[0] - pull_of_c).abs() < 1e-5,
            "c's net pull should be exactly tau/n, got {out:?}"
        );
    }

    #[test]
    fn centered_clipping_centers_the_next_round_on_the_last_output_not_the_batch_mean() {
        let agg = CenteredClippingAggregator::new(1.0);
        let batch = [
            client_delta("a", &[0.0, 0.0]),
            client_delta("b", &[2.0, 0.0]),
            client_delta("c", &[10.0, 0.0]),
        ];

        let round_1 = agg.aggregate(&batch).unwrap();
        assert!((round_1[0] - 11.0 / 3.0).abs() < 1e-5);

        // Round 2, identical batch. A stateless method would return
        // round 1's answer again. Centering on v = [11/3, 0] instead:
        //   devs  = [-11/3,0] (d=11/3), [-5/3,0] (d=5/3), [19/3,0]
        //   all d > tau=1 -> [-1,0], [-1,0], [1,0] -> sum [-1,0]
        //   v_2 = [11/3, 0] + [-1/3, 0] = [10/3, 0]
        let round_2 = agg.aggregate(&batch).unwrap();
        assert!((round_2[0] - 10.0 / 3.0).abs() < 1e-5, "got {round_2:?}");
        assert!(
            (round_2[0] - round_1[0]).abs() > 1e-3,
            "round 2 repeated round 1 — the reference is not persisting"
        );
        assert_eq!(agg.reference(), Some(round_2.clone()));
    }

    #[test]
    fn centered_clipping_degenerates_to_the_plain_mean_when_the_radius_never_binds() {
        // tau far larger than any deviation -> every scale is 1 ->
        // v + mean(u_i - v) = mean(u_i), for any v. So an unclipped
        // round is exactly an unweighted mean: [0+2+10]/3 = 4.
        let agg = CenteredClippingAggregator::new(1_000.0);
        let out = agg
            .aggregate(&[
                client_delta("a", &[0.0, 0.0]),
                client_delta("b", &[2.0, 0.0]),
                client_delta("c", &[10.0, 0.0]),
            ])
            .unwrap();

        assert!((out[0] - 4.0).abs() < 1e-5, "got {out:?}");
    }

    #[test]
    fn centered_clipping_walks_away_from_an_outlier_over_successive_rounds() {
        // Two honest clients at the origin, one attacker at [1000, 0].
        // Round 1 seeds v from the mean, which the attacker has already
        // dragged to [1000/3, 0] — the documented cost of warm-starting
        // from the batch mean. What the method guarantees is what
        // happens next: each round moves the reference back toward the
        // honest cluster by a bounded step, and never lets the attacker
        // pull it further out.
        let agg = CenteredClippingAggregator::new(1.0);
        let batch = [
            client_delta("honest-1", &[0.0]),
            client_delta("honest-2", &[0.0]),
            client_delta("attacker", &[1000.0]),
        ];

        let mut previous = f32::INFINITY;
        for round in 0..5 {
            let out = agg.aggregate(&batch).unwrap()[0];
            assert!(
                out < previous,
                "round {round} moved toward the attacker: {out} !< {previous}"
            );
            previous = out;
        }
        // Each round nets exactly (-1 - 1 + 1)/3 = -1/3 of movement,
        // whatever the attacker submits.
        assert!(
            (1000.0 / 3.0 - 5.0 / 3.0 - previous).abs() < 1e-3,
            "expected five bounded steps down from the seeded mean, got {previous}"
        );
    }

    #[test]
    fn centered_clipping_returns_a_lone_clients_update_unchanged_on_a_fresh_aggregator() {
        // Not a special case in the code — it falls out of the math. The
        // seeded reference *is* the single client's own vector, so its
        // deviation is zero and nothing is clipped.
        let agg = CenteredClippingAggregator::new(0.5);
        let out = agg.aggregate(&[client_delta("solo", &[5.0, 7.0])]).unwrap();

        assert_eq!(out, vec![5.0, 7.0]);
    }

    #[test]
    fn centered_clipping_clips_a_lone_client_once_a_reference_exists() {
        // The flip side of the test above, and the reason no `n == 1`
        // early return exists: after round one there *is* a reference,
        // so a single client cannot move the model arbitrarily either.
        let agg = CenteredClippingAggregator::new(1.0);
        agg.aggregate(&[client_delta("solo", &[0.0])]).unwrap();

        // v = [0]; a lone client at [50] deviates by 50, clipped to 1,
        // averaged over n=1 -> v_1 = [1].
        let out = agg.aggregate(&[client_delta("solo", &[50.0])]).unwrap();
        assert!((out[0] - 1.0).abs() < 1e-6, "got {out:?}");
    }

    #[test]
    fn centered_clipping_rejects_an_empty_batch() {
        let agg = CenteredClippingAggregator::new(1.0);
        assert!(matches!(
            agg.aggregate(&[]),
            Err(AggregatorError::EmptyBatch)
        ));
    }
}
