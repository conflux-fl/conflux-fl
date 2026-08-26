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

use crate::weights::decode_and_validate;
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
                for (c, x) in combined.iter_mut().zip(w) {
                    *c += x;
                }
            }
        } else {
            for (i, w) in weights.iter().enumerate() {
                for (c, x) in combined.iter_mut().zip(&decoded[i]) {
                    *c += w * x;
                }
            }
        }
        for c in &mut combined {
            *c /= n as f32;
        }

        Ok(combined)
    }
}

fn cosine_similarity_traces(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    // Compare the most recent shared window — a client with a longer
    // history than its peer is compared only over the overlap, not
    // padded with zeros (which would artificially depress similarity).
    cosine_similarity(&a[a.len() - n..], &b[b.len() - n..])
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
    pub client_id: String,
    pub stability: f32,
    pub collusion: f32,
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
            history: Mutex::new(HashMap::new()),
            last_diagnostics: Mutex::new(Vec::new()),
        }
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

        let collusion: Vec<f32> = (0..n)
            .map(|i| {
                (0..n)
                    .filter(|&j| j != i)
                    .map(|j| cosine_similarity_traces(&traces[i], &traces[j]))
                    .fold(0.0f32, f32::max)
            })
            .collect();

        let weights: Vec<f32> = (0..n)
            .map(|i| {
                let unstable = stability[i] < self.stability_threshold;
                let colluding = collusion[i] > self.collusion_threshold;
                if unstable && colluding {
                    (1.0 - collusion[i]).max(0.0)
                } else {
                    1.0
                }
            })
            .collect();

        let dim = decoded[0].len();
        let mut combined = vec![0.0f32; dim];
        let mut weight_sum = 0.0f32;
        for (i, w) in weights.iter().enumerate() {
            let effective = w * updates[i].num_samples as f32;
            weight_sum += effective;
            for (c, x) in combined.iter_mut().zip(&decoded[i]) {
                *c += effective * x;
            }
        }
        if weight_sum > 0.0 {
            for c in &mut combined {
                *c /= weight_sum;
            }
        } else {
            // Every client scored zero trust (degenerate) — fall back to
            // an unweighted mean, same pattern as `FoolsGoldAggregator`.
            for w in &decoded {
                for (c, x) in combined.iter_mut().zip(w) {
                    *c += x;
                }
            }
            for c in &mut combined {
                *c /= n as f32;
            }
        }

        *self
            .last_diagnostics
            .lock()
            .expect("DssAggregator diagnostics mutex poisoned") = updates
            .iter()
            .zip(stability.iter().zip(collusion.iter().zip(weights.iter())))
            .map(|(u, (&s, (&c, &w)))| ClientDssDiagnostic {
                client_id: u.client_id.clone(),
                stability: s,
                collusion: c,
                weight: w,
            })
            .collect();

        Ok(combined)
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
}
