//! The `trusted` family: methods anchored to a signal the server
//! computes for itself, rather than one derived from the client batch.
//!
//! Every other family in this crate reads only the batch. That is a real
//! ceiling: a colluding majority *is* the batch's consensus, so a method
//! whose only evidence is the batch has nothing left to appeal to. The
//! `trusted` family is the structural answer — it compares each client
//! against something no client can influence.
//!
//! The signal comes from outside this crate, and outside
//! `conflux-server`. The training capability lives in an optional
//! sidecar process (`conflux-trusted-reference`), so the server never
//! gains a model-architecture dependency and its model-opacity boundary
//! survives. What arrives here is a plain `f32` vector.
//!
//! # How the reference reaches an aggregator
//!
//! Via the crate's interior-mutability pattern, and this is the first
//! member to actually need it for something other than history:
//! `Aggregator::aggregate` is synchronous and has no network access, so
//! the server fetches the reference asynchronously *before* the call and
//! injects it with [`FlTrustAggregator::set_reference`]. The aggregator
//! then reads its own `Mutex` field, exactly as `FoolsGoldAggregator`
//! reads its history.
//!
//! The failure mode this shape creates is worth naming, because it is
//! handled deliberately: an aggregator whose reference was never injected
//! could quietly fall back to an unweighted mean, which is FedAvg — the
//! method FLTrust exists to replace, silently substituted at the moment
//! the defense was supposed to engage. It returns
//! [`AggregatorError::MissingTrustedReference`] instead.

use std::sync::Mutex;

use conflux_proto::ClientDelta;

use crate::weights::decode_and_validate;
use crate::{Aggregator, AggregatorError};

/// The server's own view of this round, handed to a `trusted`-family
/// aggregator before it runs.
///
/// Carries both vectors because FLTrust is defined over *updates* while
/// Conflux transmits *full weights*: the aggregator needs the global
/// model to recover `w_i − w` for each client and `w_ref − w` for the
/// reference. Passing both keeps that subtraction in one place instead of
/// splitting it between the server and the aggregator.
#[derive(Debug, Clone, PartialEq)]
pub struct TrustedReference {
    /// The global model at the start of this round — the same vector the
    /// server dispatched to clients.
    pub global_weights: Vec<f32>,
    /// What the sidecar produced by training from `global_weights` on the
    /// trusted root dataset.
    pub reference_weights: Vec<f32>,
}

/// Cosine similarity, accumulated in `f64`.
///
/// `f64` for the reason established in `temporal.rs`: a dot product of
/// two large-but-finite `f32` vectors overflows `f32` long before either
/// input is unreasonable, and `inf / inf` is `NaN` — which, compared
/// against a threshold, reads as "not suspicious" and lets exactly the
/// most extreme update through.
fn cosine_similarity_f64(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let norm_a = a.iter().map(|x| *x as f64 * *x as f64).sum::<f64>().sqrt();
    let norm_b = b.iter().map(|x| *x as f64 * *x as f64).sum::<f64>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        // One of the vectors is the zero update. Undefined rather than
        // zero, but a client that moved nowhere deserves no trust either
        // way, and `0.0` is what ReLU would make of any negative answer.
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

fn l2_norm_f64(v: &[f32]) -> f64 {
    v.iter().map(|x| *x as f64 * *x as f64).sum::<f64>().sqrt()
}

/// **FLTrust** — Cao, Fang, Liu, Jia & Gong, 2021, "FLTrust: Byzantine-
/// robust Federated Learning via Trust Bootstrapping" (NDSS).
///
/// The server trains its own update `g₀` on a small trusted root dataset,
/// then for each client update `gᵢ`:
///
/// ```text
/// TSᵢ = ReLU(cos(gᵢ, g₀))            trust score
/// ĝᵢ  = (‖g₀‖ / ‖gᵢ‖) · gᵢ           magnitude normalization
/// g   = (Σᵢ TSᵢ · ĝᵢ) / (Σᵢ TSᵢ)     the aggregate
/// ```
///
/// Two mechanisms, and both matter. The **trust score** zeroes out any
/// client pointing away from the reference — `ReLU` means a client
/// opposing the trusted direction contributes exactly nothing, not a
/// small negative amount. The **magnitude normalization** rescales every
/// surviving client to the reference's own norm, which is what stops an
/// attacker who points the right way but very hard: direction earns
/// influence, size does not.
///
/// # Why this resists what the other families cannot
///
/// Krum, Trimmed Mean, Median, FoolsGold and the rest all derive their
/// notion of "normal" from the batch. A colluding majority is therefore
/// normal by construction, which measurement bears out.
/// FLTrust never asks the batch anything: `g₀` comes from data no client
/// contributed to, so a unanimous batch of attackers is scored against
/// the same reference an honest one would be, and is rejected by the same
/// arithmetic.
///
/// The cost is the assumption that replaces it: **the root dataset must
/// be clean and representative.** FLTrust does not weaken gracefully if
/// it is not — a reference trained on unrepresentative data points
/// somewhere honest clients do not, and `ReLU` then zeroes *them*. The
/// trust bootstrap is the whole method, and it is exactly as good as the
/// data behind it.
///
/// # Fidelity notes
///
/// - The combine is FLTrust's own trust-weighted mean. It deliberately
///   does **not** use Conflux's `num_samples` weighting convention —
///   like `FoolsGoldAggregator` and `CenteredClippingAggregator`, results
///   stay directly comparable to the published experiments.
/// - The paper defines everything over updates `gᵢ = wᵢ − w`. Conflux
///   transmits full weights, so this subtracts the global model itself
///   and adds it back at the end. That is a change of representation,
///   not of algorithm: every quantity the paper defines is computed on
///   exactly the vectors it defines them on.
/// - The reference comes from a sidecar this crate cannot see. If none
///   was injected, this refuses to run rather than
///   degrading to an unweighted mean — see
///   [`AggregatorError::MissingTrustedReference`].
pub struct FlTrustAggregator {
    /// `Mutex` for the usual reason: `aggregate` takes `&self` so one
    /// aggregator serves every round behind an `Arc`, and interior
    /// mutability is how a method carries per-round state without
    /// changing the trait for the methods that need none.
    ///
    /// `None` until the server injects one, and reset is never automatic:
    /// a stale reference is caught by the round check in
    /// `set_reference`'s caller, not by clearing this optimistically.
    reference: Mutex<Option<TrustedReference>>,
}

impl Default for FlTrustAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl FlTrustAggregator {
    /// An aggregator with no reference yet. It will refuse to aggregate
    /// until one is injected.
    pub fn new() -> Self {
        Self {
            reference: Mutex::new(None),
        }
    }

    /// Supplies the round's trusted reference, normally from
    /// `conflux-server` after it has called the sidecar.
    ///
    /// Takes `&self` so it composes with the `Arc<dyn Aggregator>` the
    /// round pipeline already holds — the same reason `aggregate` does.
    pub fn set_reference(&self, reference: TrustedReference) {
        *self
            .reference
            .lock()
            .expect("FlTrustAggregator reference mutex poisoned") = Some(reference);
    }

    /// The currently-injected reference, if any. Read-only, for tests and
    /// diagnostics; `aggregate` reads the field directly.
    pub fn reference(&self) -> Option<TrustedReference> {
        self.reference
            .lock()
            .expect("FlTrustAggregator reference mutex poisoned")
            .clone()
    }
}

impl Aggregator for FlTrustAggregator {
    fn requires_trusted_reference(&self) -> bool {
        true
    }

    fn set_trusted_reference(&self, reference: TrustedReference) {
        self.set_reference(reference);
    }

    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let reference = self
            .reference
            .lock()
            .expect("FlTrustAggregator reference mutex poisoned")
            .clone()
            .ok_or(AggregatorError::MissingTrustedReference)?;

        if reference.global_weights.len() != dim || reference.reference_weights.len() != dim {
            return Err(AggregatorError::TrustedReferenceDimension {
                expected: dim,
                global: reference.global_weights.len(),
                reference: reference.reference_weights.len(),
            });
        }

        // g₀ = w_ref − w, the server's own update.
        let g0: Vec<f32> = reference
            .reference_weights
            .iter()
            .zip(&reference.global_weights)
            .map(|(r, w)| r - w)
            .collect();
        let g0_norm = l2_norm_f64(&g0);

        let mut trust_sum = 0.0f64;
        let mut combined = vec![0.0f64; dim];

        for update in &decoded {
            // gᵢ = wᵢ − w.
            let gi: Vec<f32> = update
                .iter()
                .zip(&reference.global_weights)
                .map(|(u, w)| u - w)
                .collect();

            // TSᵢ = ReLU(cos(gᵢ, g₀)).
            let trust = cosine_similarity_f64(&gi, &g0).max(0.0);
            if trust <= 0.0 {
                // Pointing away from the reference, or nowhere at all.
                // Contributes exactly nothing — that is what ReLU means
                // here, and skipping is equivalent to adding zero.
                continue;
            }

            // ĝᵢ = (‖g₀‖ / ‖gᵢ‖) · gᵢ. A client that moved nowhere has no
            // direction to rescale; it is already contributing nothing.
            let gi_norm = l2_norm_f64(&gi);
            if gi_norm == 0.0 {
                continue;
            }
            let scale = g0_norm / gi_norm;

            trust_sum += trust;
            for (acc, g) in combined.iter_mut().zip(&gi) {
                *acc += trust * scale * *g as f64;
            }
        }

        if trust_sum <= 0.0 {
            // Every client was scored out. Returning the global model
            // unchanged is the honest answer: FLTrust's judgment is that
            // nothing in this batch is trustworthy, and inventing an
            // aggregate from updates it just rejected would discard
            // exactly the judgment the method exists to make.
            return Ok(reference.global_weights);
        }

        // g = Σ TSᵢ ĝᵢ / Σ TSᵢ, then back to full weights: w + g.
        Ok(reference
            .global_weights
            .iter()
            .zip(&combined)
            .map(|(w, acc)| (*w as f64 + acc / trust_sum) as f32)
            .collect())
    }
}

/// One round's per-candidate scores from the sidecar, plus the global
/// model they were computed against.
///
/// The global weights ride along for two reasons: Zeno's `ρ‖u_i‖²`
/// regularizer needs the round's starting point to form `u_i = y_i − x`,
/// and carrying it makes the scores self-describing — a dimension check
/// can prove they belong to *this* batch rather than trusting the
/// injection order.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateScores {
    /// The global model dispatched this round — what every candidate and
    /// every score is relative to.
    pub global_weights: Vec<f32>,
    /// `(client_id, score)` as the sidecar returned them. Higher is
    /// better; the sidecar's contract is "held-out improvement over the
    /// global model".
    pub scores: Vec<(String, f32)>,
}

/// **Zeno** — Xie, Koyejo & Gupta (2019), *Zeno: Distributed Stochastic
/// Gradient Descent with Suspicion-based Fault-tolerance*, ICML.
///
/// # The method
///
/// Rank every candidate by a *suspicion score* and drop the `b` most
/// suspicious before averaging:
///
/// ```text
/// sds_i = score_i − ρ · ‖y_i − x‖²
/// keep the n − b highest, unweighted mean of the kept
/// ```
///
/// where `score_i` is the sidecar's held-out improvement for candidate
/// `i` (the paper's stochastic descendant estimate, computed on data no
/// client controls) and the `ρ‖·‖²` term penalizes magnitude, so an
/// update cannot buy a high score by taking a huge step that happens to
/// help the validation batch.
///
/// # Where it sits next to FLTrust
///
/// Both anchor to server-side data, but they consume the
/// sidecar differently, which is why the service has two RPCs. FLTrust
/// needs **one vector before the batch is known** — its reference
/// update. Zeno needs **one number per candidate**, so it can only ask
/// once the batch exists; the round pipeline calls `ScoreUpdates` after
/// the buffer flushes and injects the result here through
/// [`Aggregator::set_candidate_scores`].
///
/// # A caution this catalog already measured
///
/// A filter that *ranks* clients can make a robust base worse — the
/// FLANDERS measurement found exactly that against stable Sybils.
/// Zeno's defense against the same failure is that its ranking signal
/// comes from held-out data rather than from batch geometry: a
/// colluding majority can dominate every pairwise-distance argument,
/// but it cannot make a bad update score well on data it never saw.
/// The cost is the same one FLTrust pays — the defense is only as good
/// as the sidecar's dataset.
///
/// # Fidelity notes
///
/// - **Scores are consumed on use.** `aggregate` takes them rather than
///   reading them, so a round that was never scored fails with
///   [`AggregatorError::MissingCandidateScores`] instead of silently
///   ranking this batch with the previous batch's numbers.
/// - **An unscored candidate is an error, not a default.** The sidecar
///   may omit candidates; treating omission as any particular score
///   would let it include or exclude a client silently.
/// - **A non-finite score ranks strictly last.** It cannot be compared,
///   and "uncomparable" must never beat a real score — the same
///   direction-of-failure reasoning the reputation filter documents.
/// - `b` comes from `byzantine_fraction` exactly as the `robust` family
///   sizes it: `floor(fraction · n)`, capped at `n − 1`.
pub struct ZenoAggregator {
    /// Assumed Byzantine fraction, sizing how many candidates are
    /// dropped each round.
    pub byzantine_fraction: f32,
    /// The paper's `ρ`: how hard update magnitude is penalized. The
    /// builtin `0.0005` is the value the paper's own experiments use
    /// (their §5); like every cited default it is a starting point, not a
    /// tuned recommendation.
    pub rho: f32,
    /// This round's scores. `Mutex<Option<…>>` per the crate's pattern;
    /// `Option` because "not injected yet" is a state `aggregate` must
    /// refuse, and `take()`n on use so it cannot go stale.
    scores: Mutex<Option<CandidateScores>>,
}

impl ZenoAggregator {
    /// `byzantine_fraction` sizes the drop count; `rho` is the paper's
    /// magnitude penalty.
    pub fn new(byzantine_fraction: f32, rho: f32) -> Self {
        Self {
            byzantine_fraction,
            rho,
            scores: Mutex::new(None),
        }
    }
}

impl Aggregator for ZenoAggregator {
    fn requires_candidate_scores(&self) -> bool {
        true
    }

    fn set_candidate_scores(&self, scores: CandidateScores) {
        *self
            .scores
            .lock()
            .expect("ZenoAggregator scores mutex poisoned") = Some(scores);
    }

    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        // `take`, not `clone`: scores describe exactly one batch, and
        // the failure this prevents — ranking round N's clients with
        // round N−1's numbers — would be invisible in any output.
        let injected = self
            .scores
            .lock()
            .expect("ZenoAggregator scores mutex poisoned")
            .take()
            .ok_or(AggregatorError::MissingCandidateScores)?;

        if injected.global_weights.len() != dim {
            return Err(AggregatorError::CandidateScoresDimension {
                expected: dim,
                got: injected.global_weights.len(),
            });
        }

        let mut by_client: std::collections::HashMap<String, f32> =
            injected.scores.into_iter().collect();

        // Suspicion score per candidate. `f64` for the norm, as
        // everywhere: two finite `f32` weights can be `2·f32::MAX`
        // apart, and the whole point of the ρ term is to punish exactly
        // the updates large enough to overflow it.
        let mut ranked: Vec<(usize, f64)> = Vec::with_capacity(updates.len());
        for (i, (update, weights)) in updates.iter().zip(&decoded).enumerate() {
            let score = by_client.remove(&update.client_id).ok_or_else(|| {
                AggregatorError::UnscoredClient {
                    client_id: update.client_id.clone(),
                }
            })?;
            let norm_sq: f64 = weights
                .iter()
                .zip(&injected.global_weights)
                .map(|(y, x)| {
                    let d = *y as f64 - *x as f64;
                    d * d
                })
                .sum();
            let sds = score as f64 - self.rho as f64 * norm_sq;
            // Uncomparable must never outrank a real number.
            ranked.push((
                i,
                if sds.is_finite() {
                    sds
                } else {
                    f64::NEG_INFINITY
                },
            ));
        }

        let drop = crate::robust::byzantine_count(self.byzantine_fraction, updates.len());
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
        ranked.truncate(updates.len() - drop);

        // Unweighted mean of the kept candidates, `1/k` folded into each
        // term — the paper averages the survivors and weights none of
        // them, and the overflow discipline is the catalog's standing
        // one.
        let share = 1.0 / ranked.len() as f32;
        let mut out = vec![0.0f32; dim];
        for (i, _) in &ranked {
            for (acc, w) in out.iter_mut().zip(&decoded[*i]) {
                *acc += w * share;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conflux_proto::encode_weights;

    fn delta(client_id: &str, weights: &[f32]) -> ClientDelta {
        ClientDelta {
            client_id: client_id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples: 10,
            ..Default::default()
        }
    }

    /// Global at the origin, reference pointing at [1, 1, 1].
    fn reference() -> TrustedReference {
        TrustedReference {
            global_weights: vec![0.0, 0.0, 0.0],
            reference_weights: vec![1.0, 1.0, 1.0],
        }
    }

    #[test]
    fn without_a_reference_it_refuses_rather_than_becoming_fedavg() {
        // The most important test in this file. Silently falling back to
        // an unweighted mean would substitute the exact method FLTrust
        // replaces, at the moment the defense was meant to engage, with
        // nothing in the logs to say so.
        let aggregator = FlTrustAggregator::new();
        let err = aggregator
            .aggregate(&[delta("a", &[1.0, 1.0, 1.0])])
            .expect_err("must refuse without a reference");
        assert!(matches!(err, AggregatorError::MissingTrustedReference));
    }

    #[test]
    fn a_client_opposing_the_reference_contributes_nothing() {
        // ReLU, not a small negative weight: an attacker pointing the
        // wrong way is excluded outright.
        let aggregator = FlTrustAggregator::new();
        aggregator.set_reference(reference());

        let out = aggregator
            .aggregate(&[
                delta("honest", &[1.0, 1.0, 1.0]),
                delta("opposed", &[-1.0, -1.0, -1.0]),
            ])
            .unwrap();

        // Only the honest client counted, so the result is its own
        // direction rescaled to the reference's norm — i.e. the
        // reference itself.
        assert!(
            out.iter().all(|w| (w - 1.0).abs() < 1e-5),
            "got {out:?}, expected ~[1, 1, 1]"
        );
    }

    #[test]
    fn magnitude_does_not_buy_influence() {
        // The second FLTrust mechanism. An attacker aligned with the
        // reference but a thousand times larger must not dominate: every
        // survivor is rescaled to ‖g₀‖ first.
        let aggregator = FlTrustAggregator::new();
        aggregator.set_reference(reference());

        let out = aggregator
            .aggregate(&[
                delta("honest", &[1.0, 1.0, 1.0]),
                delta("loud", &[1000.0, 1000.0, 1000.0]),
            ])
            .unwrap();

        assert!(
            out.iter().all(|w| (w - 1.0).abs() < 1e-4),
            "got {out:?}; a 1000x update must not drag the aggregate"
        );
    }

    #[test]
    fn a_colluding_majority_does_not_win() {
        // The property no batch-derived method has. Three of four clients
        // collude on a direction opposite the reference — a clear
        // majority, and the batch's own consensus — and are still
        // excluded, because the reference never consulted the batch.
        let aggregator = FlTrustAggregator::new();
        aggregator.set_reference(reference());

        let out = aggregator
            .aggregate(&[
                delta("honest", &[1.0, 1.1, 0.9]),
                delta("sybil-1", &[-5.0, -5.0, -5.0]),
                delta("sybil-2", &[-5.1, -4.9, -5.0]),
                delta("sybil-3", &[-4.9, -5.1, -5.0]),
            ])
            .unwrap();

        assert!(
            out.iter().all(|w| *w > 0.0),
            "got {out:?}; the aggregate must follow the reference, not the majority"
        );
    }

    #[test]
    fn every_client_scored_out_returns_the_global_model_unchanged() {
        let aggregator = FlTrustAggregator::new();
        aggregator.set_reference(TrustedReference {
            global_weights: vec![7.0, 8.0, 9.0],
            reference_weights: vec![8.0, 9.0, 10.0],
        });

        let out = aggregator
            .aggregate(&[delta("a", &[6.0, 7.0, 8.0]), delta("b", &[5.0, 6.0, 7.0])])
            .unwrap();

        assert_eq!(
            out,
            vec![7.0, 8.0, 9.0],
            "nothing trustworthy means no movement, not an invented aggregate"
        );
    }

    #[test]
    fn a_reference_of_the_wrong_dimension_is_rejected() {
        let aggregator = FlTrustAggregator::new();
        aggregator.set_reference(TrustedReference {
            global_weights: vec![0.0, 0.0],
            reference_weights: vec![1.0, 1.0],
        });

        let err = aggregator
            .aggregate(&[delta("a", &[1.0, 1.0, 1.0])])
            .expect_err("a 2-weight reference cannot score a 3-weight batch");
        assert!(matches!(
            err,
            AggregatorError::TrustedReferenceDimension { .. }
        ));
    }

    #[test]
    fn extreme_but_finite_updates_do_not_produce_a_non_finite_aggregate() {
        // The standing rule, applied to this family member: an
        // aggregator may reject, but must never return a non-finite
        // value. `f32::MAX` squared overflows `f32`, which is why the
        // cosine and the norms here are computed in `f64`.
        let aggregator = FlTrustAggregator::new();
        aggregator.set_reference(reference());

        let out = aggregator
            .aggregate(&[
                delta("honest", &[1.0, 1.0, 1.0]),
                delta("huge", &[f32::MAX, f32::MAX, f32::MAX]),
            ])
            .unwrap();

        assert!(out.iter().all(|w| w.is_finite()), "got {out:?}");
    }
}

#[cfg(test)]
mod zeno_tests {
    use super::*;
    use conflux_proto::encode_weights;

    fn delta(id: &str, weights: &[f32]) -> ClientDelta {
        ClientDelta {
            client_id: id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples: 10,
            ..Default::default()
        }
    }

    fn scores(global: &[f32], pairs: &[(&str, f32)]) -> CandidateScores {
        CandidateScores {
            global_weights: global.to_vec(),
            scores: pairs.iter().map(|(c, s)| (c.to_string(), *s)).collect(),
        }
    }

    /// n = 4, fraction 0.25 → b = 1: the single lowest-scored candidate
    /// is dropped and the rest are averaged unweighted. Hand-derived:
    /// mean of a, b, c = ([1] + [2] + [3]) / 3 = [2].
    #[test]
    fn zeno_drops_the_b_lowest_scored_and_averages_the_rest() {
        let agg = ZenoAggregator::new(0.25, 0.0);
        agg.set_candidate_scores(scores(
            &[0.0],
            &[("a", 1.0), ("b", 0.9), ("c", 0.8), ("d", -5.0)],
        ));
        let out = agg
            .aggregate(&[
                delta("a", &[1.0]),
                delta("b", &[2.0]),
                delta("c", &[3.0]),
                delta("d", &[100.0]),
            ])
            .unwrap();
        assert!((out[0] - 2.0).abs() < 1e-6, "got {out:?}");
    }

    /// The ρ term is what stops a huge step from buying a good rank.
    /// Both candidates score 1.0 on held-out data; rho = 0.02 makes
    /// sds_a = 1 − 0.02·1 = 0.98 and sds_b = 1 − 0.02·100 = −1.0, so b
    /// is the one dropped despite the identical sidecar score.
    #[test]
    fn the_rho_penalty_drops_the_large_update_at_equal_sidecar_score() {
        let agg = ZenoAggregator::new(0.5, 0.02);
        agg.set_candidate_scores(scores(&[0.0, 0.0], &[("a", 1.0), ("b", 1.0)]));
        let out = agg
            .aggregate(&[delta("a", &[1.0, 0.0]), delta("b", &[10.0, 0.0])])
            .unwrap();
        assert_eq!(out, vec![1.0, 0.0]);
    }

    /// Aggregating without this round's scores must refuse, not rank.
    #[test]
    fn missing_scores_error_rather_than_silently_averaging() {
        let agg = ZenoAggregator::new(0.25, 0.0005);
        let err = agg.aggregate(&[delta("a", &[1.0])]).unwrap_err();
        assert!(matches!(err, AggregatorError::MissingCandidateScores));
    }

    /// Scores are consumed on use: a second round without a fresh
    /// injection must fail, never reuse round N−1's numbers on round N's
    /// batch — a failure that would be invisible in any output.
    #[test]
    fn scores_are_consumed_once_and_never_reused_across_rounds() {
        let agg = ZenoAggregator::new(0.0, 0.0);
        agg.set_candidate_scores(scores(&[0.0], &[("a", 1.0)]));
        agg.aggregate(&[delta("a", &[1.0])]).unwrap();

        let err = agg.aggregate(&[delta("a", &[1.0])]).unwrap_err();
        assert!(matches!(err, AggregatorError::MissingCandidateScores));
    }

    /// The sidecar may omit a candidate; omission is an error, not a
    /// default score in either direction.
    #[test]
    fn an_unscored_client_is_an_error_not_a_default() {
        let agg = ZenoAggregator::new(0.0, 0.0);
        agg.set_candidate_scores(scores(&[0.0], &[("a", 1.0)]));
        let err = agg
            .aggregate(&[delta("a", &[1.0]), delta("b", &[2.0])])
            .unwrap_err();
        match err {
            AggregatorError::UnscoredClient { client_id } => assert_eq!(client_id, "b"),
            other => panic!("expected UnscoredClient, got {other}"),
        }
    }

    /// Scores computed against a different model must not rank this
    /// batch.
    #[test]
    fn a_dimension_mismatch_between_scores_and_batch_errors() {
        let agg = ZenoAggregator::new(0.0, 0.0);
        agg.set_candidate_scores(scores(&[0.0, 0.0, 0.0], &[("a", 1.0)]));
        let err = agg.aggregate(&[delta("a", &[1.0])]).unwrap_err();
        assert!(matches!(
            err,
            AggregatorError::CandidateScoresDimension {
                expected: 1,
                got: 3
            }
        ));
    }

    /// A NaN score is uncomparable, and uncomparable must rank strictly
    /// last — the same direction-of-failure rule the reputation filter
    /// documents, applied here.
    #[test]
    fn a_non_finite_score_ranks_last_and_is_dropped_first() {
        let agg = ZenoAggregator::new(0.34, 0.0);
        agg.set_candidate_scores(scores(&[0.0], &[("a", f32::NAN), ("b", 0.1), ("c", 0.2)]));
        let out = agg
            .aggregate(&[delta("a", &[100.0]), delta("b", &[1.0]), delta("c", &[3.0])])
            .unwrap();
        // b = floor(0.34 * 3) = 1 → NaN-scored `a` is the drop.
        assert!((out[0] - 2.0).abs() < 1e-6, "got {out:?}");
    }

    /// fraction 0 keeps everyone: Zeno degenerates to the unweighted
    /// mean, exactly as the paper's b = 0 does.
    #[test]
    fn zero_byzantine_fraction_is_the_plain_unweighted_mean() {
        let agg = ZenoAggregator::new(0.0, 0.0);
        agg.set_candidate_scores(scores(&[0.0], &[("a", -1.0), ("b", 1.0)]));
        let out = agg
            .aggregate(&[delta("a", &[0.0]), delta("b", &[4.0])])
            .unwrap();
        assert!((out[0] - 2.0).abs() < 1e-6, "got {out:?}");
    }

    #[test]
    fn empty_batch_errors() {
        let agg = ZenoAggregator::new(0.25, 0.0005);
        assert!(matches!(
            agg.aggregate(&[]),
            Err(AggregatorError::EmptyBatch)
        ));
    }

    /// The two trusted-family methods consume the sidecar differently,
    /// and each must only ask for what it uses.
    #[test]
    fn capability_flags_are_disjoint_across_the_trusted_family() {
        let zeno = ZenoAggregator::new(0.25, 0.0005);
        assert!(zeno.requires_candidate_scores());
        assert!(!zeno.requires_trusted_reference());

        let fltrust = FlTrustAggregator::new();
        assert!(fltrust.requires_trusted_reference());
        assert!(!fltrust.requires_candidate_scores());

        assert!(!crate::FedAvg::default().requires_candidate_scores());
    }
}
