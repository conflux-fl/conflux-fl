//! Cross-round, history-aware aggregation — methods that need memory
//! across rounds, unlike every member of `averaging`/`robust`, which
//! judges each round's batch in isolation. See
//! `docs/research/temporal-consistency-aggregation.md` for why this
//! family exists: no stateless method can distinguish a colluding Sybil
//! cluster from a legitimate majority within a single round, since both
//! can produce the same single-round batch geometry by construction.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use conflux_proto::ClientDelta;

use crate::weights::{accumulate_scaled_difference, accumulate_weighted, decode_and_validate};
use crate::{Aggregator, AggregatorError};

fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
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
/// underspecified enough that a from-scratch reading previously got the
/// pardoning direction backwards (see git history). Kept as its own
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
/// of problem Phase 7d solved for the privacy accountant, not solved
/// here, and a real follow-up before this is relied on across restarts.
///
/// **Fidelity note (ADR 0008)**: `foolsgold_weights` above is a direct
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
        if all_zero {
            // Degenerate case `foolsgold_weights` itself falls back on
            // (see its own doc comment) — an unweighted mean rather than
            // producing an all-zero aggregate.
            for w in &decoded {
                accumulate_weighted(&mut combined, w, 1.0);
            }
        } else {
            for (i, w) in weights.iter().enumerate() {
                accumulate_weighted(&mut combined, &decoded[i], *w);
            }
        }
        for c in &mut combined {
            *c /= n as f32;
        }

        Ok(combined)
    }
}

/// Cosine similarity between two deviation traces, **in `f64`**.
///
/// The precision is the point, and it is not a micro-optimization in
/// reverse — it is a correctness fix. DSS turns this score into a weight
/// with `weight = 1 − collusion`, and when two traces are nearly
/// parallel the score sits just under `1.0`, so that subtraction is
/// catastrophic cancellation: in `f32`, `1.0 − 0.999998` leaves barely
/// one significant digit, and the surviving digit is rounding noise.
/// `docs/research/temporal-consistency-aggregation.md` §5.8 measured the
/// consequence — every client's weight collapsing into the `1e-7`–`1e-5`
/// band with an essentially arbitrary ordering, so whichever client
/// happened to hold the largest meaningless value decided the round.
///
/// Computed in `f64`, the same subtraction retains around ten
/// significant digits, which is the difference between a weight that
/// encodes a real (if very fine) trust judgment and one that encodes
/// float noise.
///
/// `cosine_similarity` (the `f32` version) is deliberately left alone:
/// `FoolsGoldAggregator` uses it, and that function is a line-by-line
/// translation of the FoolsGold authors' own reference implementation
/// (ADR 0008). Changing its arithmetic would silently make this
/// codebase's FoolsGold something other than the published FoolsGold.
fn cosine_similarity_traces_f64(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    // Compare the most recent shared window — a client with a longer
    // history than its peer is compared only over the overlap, not
    // padded with zeros (which would artificially depress similarity).
    let a = &a[a.len() - n..];
    let b = &b[b.len() - n..];

    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let norm_a = a.iter().map(|x| *x as f64 * *x as f64).sum::<f64>().sqrt();
    let norm_b = b.iter().map(|x| *x as f64 * *x as f64).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// **Deviation Stability Scoring (DSS)** — a research hypothesis from
/// `docs/research/temporal-consistency-aggregation.md` §6, **not a
/// validated method and never a framework default**. Per this project's
/// governing principle (faithful catalog of published methods, never a
/// framework-imposed "safe default" — see `docs/phases/
/// phase-13-reputation-reference-fix.md`'s "Revision history"), this
/// type is deliberately **not** registered in `build_aggregator`'s
/// string-based catalog: it's constructed directly
/// (`DssAggregator::new(base)`) by whoever is running an experiment
/// against it, the same way `docs/research/scripts/` already drives
/// `FoolsGoldAggregator` and the attacks in `conflux-attacks` directly
/// rather than through the production config path.
///
/// **Implementation note — one simplification versus §6.2's original
/// sketch**: that section described DSS as modulating a wrapped
/// method's own *internal* per-client weight. `Aggregator` has no way to
/// expose that (it only returns the combined `Vec<f32>`), and adding one
/// would mean changing the trait for all eleven existing implementations
/// to serve a method that isn't validated yet — exactly the kind of
/// premature framework change this project's governing principle warns
/// against. Implemented here instead as: run `base` to get a reference
/// point, measure each client's *distance from that reference* as the
/// deviation signal (uniform across every base method, matching how
/// `docs/research/temporal-consistency-aggregation.md`'s own Experiment
/// 2.3 already treats every `Aggregator` as a black box), then combine
/// the *original* submissions with DSS's own stability/collusion-derived
/// weights — the base method informs the reference point, not the final
/// combine.
///
/// Mechanism (§6.2): each client's distance from `base`'s output is
/// tracked in a rolling window; **stability** is `1 / (1 + Var(trace))`;
/// **collusion** is the max pairwise cosine similarity between a
/// client's deviation trace and any other client's; a client's weight is
/// reduced only when *both* stability is low *and* collusion is high —
/// deliberately not penalizing a merely-erratic-but-non-colluding client
/// on its own, which would reintroduce Claim 2's problem in a new form.
/// The combine step renormalizes by the sum of final weights, not raw
/// client count `n` — the correction Experiment 2.2's Finding 1
/// (`docs/research/temporal-consistency-aggregation.md` §5.3) found
/// FoolsGold's own reference implementation needed and doesn't have.
/// Encodes a DSS weight into a `num_samples` count the base method can
/// act on.
///
/// `num_samples` is an integer, and every method that reads it
/// (`SampleCountWeighting`, i.e. FedAvg) only ever uses it as a *ratio*
/// against the batch's total — so the absolute scale is free, and
/// multiplying through by a large fixed factor before truncating is what
/// keeps a fractional weight from rounding away. Without it, weight
/// `0.37` on `num_samples = 10` truncates to `3`, a 19% distortion of
/// the judgment DSS just made.
///
/// Clamped to at least `1` for any surviving client: a client that
/// passed the `w > 0.0` filter is one DSS chose to keep, and letting it
/// round to zero here would silently re-exclude it — and, if every
/// survivor did so, hand the base method a batch summing to zero.
fn scale_samples(num_samples: u64, weight: f32) -> u64 {
    const PRECISION: f64 = 1e6;
    let scaled = weight as f64 * num_samples as f64 * PRECISION;
    if !scaled.is_finite() || scaled <= 0.0 {
        return 1;
    }
    (scaled as u64).max(1)
}

/// Deviation Stability Scoring — see the module-level notes above for
/// what it is and why it is deliberately not in the shipped catalog.
pub struct DssAggregator {
    base: Box<dyn Aggregator>,
    /// How many recent rounds' deviation values each client's trace
    /// keeps — the "temporal" window the stability/collusion scores are
    /// computed over.
    pub window: usize,
    /// Below this, a client's deviation trace counts as "unstable."
    pub stability_threshold: f32,
    /// Above this, a client's trace counts as "suspiciously similar to
    /// some other client's."
    pub collusion_threshold: f32,
    /// Whether the final combine runs *through* the wrapped base method
    /// (the default, `true`) or is DSS's own weighted mean over every
    /// raw submission (`false` — the original behavior, kept only so the
    /// two can be compared within a single sweep).
    ///
    /// This is Finding 3's fix. With `false`, DSS uses the base method
    /// solely to compute a deviation reference and then combines the raw
    /// batch itself — so whenever DSS's own gate doesn't fire, every
    /// weight is `1.0` and the result is a plain weighted mean,
    /// **discarding whatever the base method would have excluded**. That
    /// is why `dss_krum` measured ~57x worse than plain `krum` against
    /// `persistent_sybil` (§5.5 of the research doc): stable colluders
    /// never trip DSS's gate, so wrapping Krum silently replaced Krum
    /// with FedAvg.
    ///
    /// With `true`, DSS applies its judgment by re-weighting the batch
    /// and handing it back to the base method, which then runs its own
    /// algorithm on it — Krum still selects, Trimmed Mean still trims. A
    /// non-firing gate now degrades to *the base method*, which is the
    /// floor a wrapper should have had all along.
    pub combine_through_base: bool,
    history: Mutex<HashMap<String, VecDeque<f32>>>,
    /// Per-client (stability, collusion, weight) from the most recent
    /// `aggregate()` call — pure diagnostics, read by
    /// `last_diagnostics()`, never consulted by `aggregate()` itself.
    /// Exists so experiments (`docs/research/
    /// temporal-consistency-aggregation.md` §5.6/§5.7) can inspect *why*
    /// a client was or wasn't down-weighted without reconstructing it via
    /// leave-one-out re-aggregation, which would corrupt a stateful
    /// aggregator's own history the moment a client is counterfactually
    /// dropped from one round's batch.
    last_diagnostics: Mutex<Vec<ClientDssDiagnostic>>,
}

/// One client's DSS internals for the most recent round — see
/// `DssAggregator::last_diagnostics`.
#[derive(Debug, Clone)]
pub struct ClientDssDiagnostic {
    /// Which client this row describes.
    pub client_id: String,
    /// `1 / (1 + Var(trace))` over the rolling window. Near 1 means a
    /// steady deviation trace; near 0 means an erratic one.
    pub stability: f32,
    /// Highest cosine similarity between this client's deviation trace
    /// and any other client's.
    pub collusion: f32,
    /// The weight this client actually received in the combine step.
    pub weight: f32,
}

impl DssAggregator {
    /// `stability_threshold`/`collusion_threshold` defaults are
    /// unvalidated starting points, not tuned constants — this whole
    /// type is a hypothesis (§6.4's own "what's explicitly not
    /// claimed"), and these are exactly the knobs
    /// `docs/research/temporal-consistency-aggregation.md`'s planned
    /// ablations (§7.3) would sweep.
    pub fn new(base: Box<dyn Aggregator>) -> Self {
        Self {
            base,
            window: 5,
            stability_threshold: 0.5,
            collusion_threshold: 0.8,
            combine_through_base: true,
            history: Mutex::new(HashMap::new()),
            last_diagnostics: Mutex::new(Vec::new()),
        }
    }

    fn record_diagnostics(
        &self,
        updates: &[ClientDelta],
        stability: &[f32],
        collusion: &[f64],
        weights: &[f32],
    ) {
        *self
            .last_diagnostics
            .lock()
            .expect("DssAggregator diagnostics mutex poisoned") = updates
            .iter()
            .zip(stability.iter().zip(collusion.iter().zip(weights.iter())))
            .map(|(u, (&s, (&c, &w)))| ClientDssDiagnostic {
                client_id: u.client_id.clone(),
                stability: s,
                // Narrowed for reporting only. The weight was computed
                // from the full-precision value; this field is for
                // humans reading a trace, not for arithmetic.
                collusion: c as f32,
                weight: w,
            })
            .collect();
    }

    /// Snapshot of each client's (stability, collusion, weight) from the
    /// most recent `aggregate()` call, in the same order as that call's
    /// `updates` slice. Empty before the first call.
    pub fn last_diagnostics(&self) -> Vec<ClientDssDiagnostic> {
        self.last_diagnostics
            .lock()
            .expect("DssAggregator diagnostics mutex poisoned")
            .clone()
    }
}

impl Aggregator for DssAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let base_result = self.base.aggregate(updates)?;
        let n = updates.len();

        let traces: Vec<Vec<f32>> = {
            let mut history = self
                .history
                .lock()
                .expect("DssAggregator history mutex poisoned");
            updates
                .iter()
                .zip(&decoded)
                .map(|(u, w)| {
                    let deviation = l2_distance(w, &base_result);
                    let entry = history.entry(u.client_id.clone()).or_default();
                    entry.push_back(deviation);
                    if entry.len() > self.window {
                        entry.pop_front();
                    }
                    entry.iter().copied().collect()
                })
                .collect()
        };

        let stability: Vec<f32> = traces
            .iter()
            .map(|trace| {
                if trace.len() < 2 {
                    // Not enough history yet to judge — no penalty for a
                    // client we've only just started observing.
                    return 1.0;
                }
                let mean = trace.iter().sum::<f32>() / trace.len() as f32;
                let variance =
                    trace.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / trace.len() as f32;
                1.0 / (1.0 + variance)
            })
            .collect();

        // Kept in `f64` all the way to the subtraction below — see
        // `cosine_similarity_traces_f64` for why that is the whole point.
        let collusion: Vec<f64> = (0..n)
            .map(|i| {
                (0..n)
                    .filter(|&j| j != i)
                    .map(|j| cosine_similarity_traces_f64(&traces[i], &traces[j]))
                    .fold(0.0f64, f64::max)
            })
            .collect();

        let weights: Vec<f32> = (0..n)
            .map(|i| {
                let unstable = stability[i] < self.stability_threshold;
                let colluding = collusion[i] > self.collusion_threshold as f64;
                if unstable && colluding {
                    // The cancellation happens here, in `f64`, and only
                    // the result is narrowed to `f32`.
                    (1.0 - collusion[i]).max(0.0) as f32
                } else {
                    1.0
                }
            })
            .collect();

        let dim = decoded[0].len();

        // Finding 3's fix: hand DSS's judgment back to the base method
        // rather than acting on it here.
        //
        // The batch below carries DSS's weights as `num_samples`
        // scaling, with fully-distrusted clients dropped outright.
        // Dropping matters as much as scaling: a selection-based base
        // (Krum, Multi-Krum, FABA, Bulyan) ignores `num_samples`
        // entirely, so a client scaled to zero would still be a
        // *candidate* it could pick. Removing it is the only way the
        // judgment reaches those methods at all.
        if self.combine_through_base {
            let reweighted: Vec<ClientDelta> = updates
                .iter()
                .zip(&weights)
                .filter(|(_, w)| **w > 0.0)
                .map(|(u, &w)| ClientDelta {
                    num_samples: scale_samples(u.num_samples, w),
                    ..u.clone()
                })
                .collect();

            let combined = if reweighted.is_empty() {
                // Every client fully distrusted — the same degenerate
                // case the original combine had, answered the same way:
                // a stable unweighted mean beats returning nothing.
                let mut mean = vec![0.0f32; dim];
                for w in &decoded {
                    accumulate_weighted(&mut mean, w, 1.0);
                }
                for m in &mut mean {
                    *m /= n as f32;
                }
                mean
            } else {
                // The base method's own algorithm, run on the batch DSS
                // has re-weighted. Errors propagate rather than being
                // swallowed: if the base can't aggregate this batch,
                // that is a real failure, not something to paper over
                // with a mean.
                self.base.aggregate(&reweighted)?
            };

            self.record_diagnostics(updates, &stability, &collusion, &weights);
            return Ok(combined);
        }

        // Original combine (`combine_through_base = false`), kept for
        // the A/B comparison in `docs/research/`'s Experiment 2.8.
        let mut combined = vec![0.0f32; dim];
        let mut weight_sum = 0.0f32;
        for (i, w) in weights.iter().enumerate() {
            let effective = w * updates[i].num_samples as f32;
            weight_sum += effective;
            accumulate_weighted(&mut combined, &decoded[i], effective);
        }

        if weight_sum > 0.0 {
            for c in &mut combined {
                *c /= weight_sum;
            }
        } else {
            // Nothing meaningfully distinguishes any client — fall back
            // to a stable unweighted mean, same pattern as
            // `FoolsGoldAggregator`.
            //
            // Reset before re-accumulating. Reaching this branch means
            // every `effective` was exactly `0.0`, so `combined` is
            // already the zero vector and this is a no-op today — kept
            // so the branch is correct on its own terms rather than
            // because of what the condition above happens to imply.
            combined.iter_mut().for_each(|c| *c = 0.0);
            for w in &decoded {
                accumulate_weighted(&mut combined, w, 1.0);
            }
            for c in &mut combined {
                *c /= n as f32;
            }
        }

        self.record_diagnostics(updates, &stability, &collusion, &weights);
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
/// **Fidelity notes (ADR 0008):**
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
///   never a hardcoded constant. **Measured (§5.13): at the framework's
///   placeholder `τ = 1.0`, this method scored *worse than no defense*
///   on a real 50,890-parameter model, and no `τ` in a 1→100 sweep
///   reached a selection-based method's accuracy.** `τ` bounds an L2
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
                let mut mean = vec![0.0f32; dim];
                for w in &decoded {
                    accumulate_weighted(&mut mean, w, 1.0);
                }
                for m in &mut mean {
                    *m /= n as f32;
                }
                mean
            }
        };

        let mut clipped_sum = vec![0.0f32; dim];
        for w in &decoded {
            let distance = l2_distance(w, &v);
            // `min(1, τ/d)`, with the `d == 0` case handled separately
            // because `τ/0` is a division by zero — though the deviation
            // it would scale is the zero vector either way, so the value
            // chosen there cannot affect the result.
            let scale = if distance > 0.0 {
                (self.clip_radius / distance).min(1.0)
            } else {
                1.0
            };
            accumulate_scaled_difference(&mut clipped_sum, w, &v, scale);
        }

        let next: Vec<f32> = v
            .iter()
            .zip(&clipped_sum)
            .map(|(vi, acc)| vi + acc / n as f32)
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

    // --- DSS (Deviation Stability Scoring) ---

    fn fedavg_base() -> Box<dyn Aggregator> {
        Box::new(crate::FedAvg::default())
    }

    #[test]
    fn dss_single_client_returns_it_unchanged() {
        let dss = DssAggregator::new(fedavg_base());
        let updates = vec![client_delta("only", &[3.0, -1.0])];

        let result = dss.aggregate(&updates).unwrap();

        assert_eq!(result, vec![3.0, -1.0]);
    }

    #[test]
    fn dss_empty_batch_errors() {
        let dss = DssAggregator::new(fedavg_base());

        let err = dss.aggregate(&[]).unwrap_err();

        assert!(matches!(err, AggregatorError::EmptyBatch));
    }

    #[test]
    fn dss_protects_a_stable_non_iid_client_even_though_it_deviates_a_lot() {
        // A client that consistently, predictably differs from the pack
        // every round should never be penalized — DSS's core claim
        // (docs/research/temporal-consistency-aggregation.md §6.1):
        // stability alone, regardless of deviation magnitude, keeps a
        // client's weight at 1.0, since the "unstable AND colluding"
        // rule only fires when a client is *also* erratic.
        let dss = DssAggregator::new(fedavg_base());
        let round = || {
            vec![
                client_delta("stable-honest-1", &[1.0, 0.0]),
                client_delta("stable-honest-2", &[0.0, 1.0]),
                client_delta("stable-non-iid", &[5.0, 5.0]), // consistently far from the pack
            ]
        };

        // Several identical rounds -- this client's deviation trace is
        // constant (zero variance), so its stability score stays at the
        // maximum regardless of how far [5,5] is from the other two.
        let mut result = dss.aggregate(&round()).unwrap();
        for _ in 0..4 {
            result = dss.aggregate(&round()).unwrap();
        }

        // With every client at weight 1.0 (nothing triggers the
        // penalty), this should match the plain FedAvg mean of all
        // three: ([1,0]+[0,1]+[5,5])/3 = [2.0, 2.0].
        assert!(
            (result[0] - 2.0).abs() < 0.05 && (result[1] - 2.0).abs() < 0.05,
            "got {result:?}, expected close to the unweighted mean [2.0, 2.0] \
             (a stable client should never be penalized, however far it deviates)"
        );
    }

    #[test]
    fn dss_last_diagnostics_matches_the_stable_client_case() {
        // Same scenario as `dss_protects_a_stable_non_iid_client_...`
        // above, checked from the diagnostics side instead of only the
        // combined result: every client's own reported weight should be
        // 1.0 (nothing penalized), in `updates` order, and the
        // stable-non-iid client's own stability score should be at or
        // near the maximum despite its large deviation.
        let dss = DssAggregator::new(fedavg_base());
        let round = || {
            vec![
                client_delta("stable-honest-1", &[1.0, 0.0]),
                client_delta("stable-honest-2", &[0.0, 1.0]),
                client_delta("stable-non-iid", &[5.0, 5.0]),
            ]
        };
        for _ in 0..5 {
            dss.aggregate(&round()).unwrap();
        }
        let diagnostics = dss.last_diagnostics();
        assert_eq!(diagnostics.len(), 3);
        for d in &diagnostics {
            assert!(
                (d.weight - 1.0).abs() < 1e-6,
                "{}: expected weight 1.0, got {}",
                d.client_id,
                d.weight
            );
        }
        let non_iid = diagnostics
            .iter()
            .find(|d| d.client_id == "stable-non-iid")
            .unwrap();
        assert!(
            non_iid.stability > 0.99,
            "stable-non-iid's stability should be near 1.0 (zero-variance trace), got {}",
            non_iid.stability
        );
    }

    #[test]
    fn wrapping_a_robust_base_no_longer_discards_its_selection() {
        // Finding 3, pinned. Stable colluders never trip DSS's
        // "unstable AND colluding" gate — their deviation trace has low
        // variance by construction — so every weight stays 1.0 and DSS
        // has no opinion to apply. The question is what it does then.
        //
        // Before this fix, it combined the raw batch itself, which is a
        // plain weighted mean: wrapping Krum silently replaced Krum with
        // FedAvg, and the sybils won. Now the re-weighted batch goes
        // back through Krum, which selects as it always would.
        use crate::{FedAvg, FilteredAggregator, KrumFilter};

        fn krum() -> Box<dyn Aggregator> {
            Box::new(FilteredAggregator::new(
                KrumFilter {
                    byzantine_fraction: 0.2,
                },
                FedAvg::default(),
            ))
        }

        // Three honest clients clustered near [1, 1]; two sybils sitting
        // together far away, submitting the *same* value every round so
        // they read as perfectly stable.
        let batch = || {
            vec![
                client_delta("honest-1", &[1.0, 1.0]),
                client_delta("honest-2", &[1.1, 0.9]),
                client_delta("honest-3", &[0.9, 1.1]),
                client_delta("sybil-1", &[50.0, 50.0]),
                client_delta("sybil-2", &[50.0, 50.0]),
            ]
        };

        let plain = krum().aggregate(&batch()).unwrap();

        let through_base = DssAggregator::new(krum());
        let mut raw = DssAggregator::new(krum());
        raw.combine_through_base = false;

        let mut through_result = vec![];
        let mut raw_result = vec![];
        for _ in 0..6 {
            through_result = through_base.aggregate(&batch()).unwrap();
            raw_result = raw.aggregate(&batch()).unwrap();
        }

        // Plain Krum excludes the sybils outright.
        assert!(
            plain[0] < 2.0,
            "plain krum should sit near the honest cluster, got {plain:?}"
        );

        // The old combine threw that away and landed near the raw mean,
        // which the sybils dominate.
        assert!(
            raw_result[0] > 10.0,
            "the original combine should reproduce Finding 3 (sybil-dominated), got {raw_result:?}"
        );

        // The fix keeps Krum's own answer.
        assert!(
            through_result[0] < 2.0,
            "combining through the base should preserve krum's exclusion, got {through_result:?}"
        );
    }

    #[test]
    fn dss_down_weights_a_pair_that_is_both_erratic_and_mutually_identical() {
        // Two colluders submitting identical-to-each-other values that
        // vary erratically round to round (both unstable AND perfectly
        // correlated with each other) -- the case the "unstable AND
        // colluding" rule is meant to catch. Two stable honest clients
        // for contrast.
        let dss = DssAggregator::new(fedavg_base());
        // Large, wildly varying swings, so the sybils' own deviation
        // trace has high variance (unstable) on top of being perfectly
        // correlated with each other (identical every round) -- both
        // conditions the penalty rule requires.
        let rounds: [[f32; 2]; 4] = [[5.0, 5.0], [-80.0, 60.0], [100.0, -90.0], [6.0, 6.0]];

        let mut result = vec![];
        for sybil_value in rounds {
            let batch = vec![
                client_delta("stable-honest-1", &[1.0, 0.0]),
                client_delta("stable-honest-2", &[0.0, 1.0]),
                client_delta("sybil-1", &sybil_value),
                client_delta("sybil-2", &sybil_value),
            ];
            result = dss.aggregate(&batch).unwrap();
        }

        // Plain FedAvg of the last round would be
        // ([1,0]+[0,1]+[6,6]+[6,6])/4 = [3.25, 3.25] -- pulled heavily
        // toward the sybils. DSS should land much closer to the honest
        // pair's own consensus around [0.5, 0.5].
        assert!(
            result[0] < 2.0 && result[1] < 2.0,
            "got {result:?}, expected pulled back toward the honest clients \
             (unweighted mean would be [3.25, 3.25])"
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
