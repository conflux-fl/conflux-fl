//! The `optimization` family: server-side adaptive optimizers applied to
//! the aggregated update.
//!
//! Every other family in this crate answers "which clients should count,
//! and how much?" This one answers a different question entirely — given
//! whatever the batch aggregated to, **how far should the server actually
//! move?** It is orthogonal to robustness, and composes with it: the
//! aggregation step is still whatever you configured, and the optimizer
//! wraps the result.
//!
//! This was the largest gap in Conflux's catalog. The robust families
//! ship twelve methods; the optimization family shipped none, while every
//! comparable framework carries several. That mattered because adaptive
//! server optimization is what makes federated training converge on
//! heterogeneous (non-IID) client data, which is the setting federated
//! learning exists for.
//!
//! # Cross-round state
//!
//! These methods are stateful by definition — the moment estimates *are*
//! the method. They follow ADR 0012's standing pattern (`Mutex` fields,
//! `&self`), the same as `temporal.rs`'s members.

use std::sync::Mutex;

use conflux_proto::ClientDelta;

use crate::weights::decode_and_validate;
use crate::{Aggregator, AggregatorError};

/// Which second-moment rule the server optimizer uses.
///
/// The three variants differ in exactly one line of Reddi et al.'s
/// Algorithm 2 (lines 12–14) and nothing else, which is why they are one
/// type with a discriminant rather than three near-identical structs —
/// the family pattern (ADR 0002) applied to the smallest thing that
/// actually varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FedOptVariant {
    /// `v_t = v_{t-1} + Δ_t²` — accumulates without decay, so the
    /// effective step size only ever shrinks. Reddi et al. set
    /// `β1 = β2 = 0` for this one, "as typical versions of Adagrad do
    /// not use momentum".
    Adagrad,
    /// `v_t = β2 v_{t-1} + (1 − β2) Δ_t²` — exponential decay, the
    /// familiar Adam second moment.
    Adam,
    /// `v_t = v_{t-1} − (1 − β2) Δ_t² sign(v_{t-1} − Δ_t²)`.
    ///
    /// The sign term is the whole point of Yogi: `v` decreases only when
    /// the incoming squared gradient is *larger* than the running
    /// estimate, so a sudden large update cannot collapse the effective
    /// learning rate the way Adam's multiplicative decay can. In a
    /// federated setting where one round's batch can look very different
    /// from the last, that difference is not academic.
    Yogi,
}

impl FedOptVariant {
    /// The paper's own defaults for `(β1, β2)` for this variant.
    ///
    /// Adagrad's `(0, 0)` is not an oversight — Reddi et al. §"we set
    /// β1 = β2 = 0 (as typical versions of Adagrad do not use
    /// momentum)". With `β1 = 0`, `m_t = Δ_t` and the momentum term
    /// vanishes, which is the intended behavior.
    pub fn paper_defaults(self) -> (f32, f32) {
        match self {
            FedOptVariant::Adagrad => (0.0, 0.0),
            FedOptVariant::Adam | FedOptVariant::Yogi => (0.9, 0.99),
        }
    }

    /// The catalog name this variant is selected by.
    pub fn name(self) -> &'static str {
        match self {
            FedOptVariant::Adagrad => "fedadagrad",
            FedOptVariant::Adam => "fedadam",
            FedOptVariant::Yogi => "fedyogi",
        }
    }
}

/// The server optimizer's own hyperparameters.
#[derive(Debug, Clone, Copy)]
pub struct FedOptParams {
    /// Server learning rate `η`. Reddi et al. tune this per task and
    /// report no single default, because there is not one — it is the
    /// parameter their entire experimental section sweeps.
    pub server_learning_rate: f32,
    /// First-moment decay `β1`.
    pub beta1: f32,
    /// Second-moment decay `β2`. Unused by [`FedOptVariant::Adagrad`],
    /// whose accumulation has no decay term.
    pub beta2: f32,
    /// Adaptivity `τ`, the denominator floor. The paper uses `1e-3`
    /// throughout and notes it "works almost as well as all other
    /// values" across tasks — one of the few genuinely safe defaults
    /// here.
    pub tau: f32,
}

impl Default for FedOptParams {
    fn default() -> Self {
        Self {
            // Reddi et al. do not publish a universal η. `1.0` is chosen
            // because it makes the optimizer a no-op scale on the
            // pseudo-gradient rather than an unannounced rescaling: at
            // η = 1 the server moves roughly as far as the aggregate
            // suggested. It is a starting point to sweep from, not a
            // recommendation — the same honest-placeholder posture
            // `clip_radius` carries, and for the same reason.
            server_learning_rate: 1.0,
            beta1: 0.9,
            beta2: 0.99,
            tau: 1e-3,
        }
    }
}

impl FedOptParams {
    /// The paper's defaults for a given variant: `τ = 1e-3` and the
    /// variant's own `(β1, β2)`.
    pub fn for_variant(variant: FedOptVariant) -> Self {
        let (beta1, beta2) = variant.paper_defaults();
        Self {
            beta1,
            beta2,
            ..Default::default()
        }
    }
}

/// **FedOpt** — Reddi, Charles, Zaheer, Garrett, Rush, Konečný, Kumar &
/// McMahan, 2021, "Adaptive Federated Optimization" (ICLR), Algorithm 2.
///
/// FedAvg takes the aggregated client update and applies it to the global
/// model directly — a server-side SGD step with learning rate 1. FedOpt
/// observes that this is a choice, not a necessity, and replaces it with
/// an adaptive optimizer:
///
/// ```text
/// Δ_t = (1/|S|) Σ_i (x_i − x_t)              the pseudo-gradient
/// m_t = β1 m_{t-1} + (1 − β1) Δ_t            first moment
/// v_t = v_{t-1} + Δ_t²                       (FedAdagrad)
/// v_t = v_{t-1} − (1 − β2) Δ_t² sign(v_{t-1} − Δ_t²)   (FedYogi)
/// v_t = β2 v_{t-1} + (1 − β2) Δ_t²           (FedAdam)
/// x_{t+1} = x_t + η · m_t / (√v_t + τ)
/// ```
///
/// The per-coordinate division is what makes it adaptive: a parameter
/// that has been receiving consistently large updates gets a *smaller*
/// effective step, and one that has barely moved gets a larger one. Under
/// non-IID client data — where different clients push different
/// coordinates hard — that is exactly the correction FedAvg lacks.
///
/// # Fidelity notes (ADR 0008)
///
/// - **`Δ_t` is the unweighted mean of client deltas**, matching
///   Algorithm 2 line 10 literally. It is *not* `num_samples`-weighted,
///   so it deliberately departs from Conflux's usual FedAvg convention —
///   as `FoolsGoldAggregator` and `CenteredClippingAggregator` also do,
///   and for the same reason: results stay comparable to the published
///   experiments. The paper does give an example-weighted alternative
///   (its Algorithm 5); Algorithm 2 is what the catalog names build, and
///   the weighted variant is not implemented rather than being silently
///   substituted.
/// - **Wrapping a different base is an extension, not the paper.**
///   [`FedOptAggregator::with_base`] lets `Δ_t` come from any aggregator
///   — Krum, say, giving a Byzantine-robust pseudo-gradient with adaptive
///   server optimization on top. That composition is not in Reddi et al.
///   and is not what the catalog names build; it is available because
///   ADR 0012 recommended the wrapping shape, and it is labelled as a
///   deviation wherever it is used.
/// - **`x_t` is tracked internally, not supplied.** The server
///   checkpoints exactly what `aggregate` returns and dispatches it next
///   round, so this aggregator's own previous output *is* `x_t`. The
///   first round has no previous output and therefore no pseudo-gradient
///   to speak of: it returns the base aggregate unchanged and seeds
///   `x_t` from it, which is Algorithm 2's `x_0` initialization.
/// - **`v_{-1} = τ²`**, the smallest value Algorithm 2's initialization
///   condition (`v_{-1} ≥ τ²`) permits.
/// - **State does not survive a restart.** `m`, `v`, and `x_t` are
///   in-process, like every other stateful method here
///   (`FoolsGoldAggregator`, `CenteredClippingAggregator`,
///   `DssAggregator`). A restarted server resumes from its checkpoint
///   with fresh moment estimates, which costs a few rounds of adaptivity
///   rather than correctness.
pub struct FedOptAggregator {
    variant: FedOptVariant,
    params: FedOptParams,
    /// `None` means the paper's unweighted mean; `Some` is the
    /// documented wrapping extension.
    base: Option<Box<dyn Aggregator>>,
    /// `(x_t, m_t, v_t)`, all `None` until the first round establishes
    /// them. One `Mutex` rather than three so a round's update is
    /// atomic — a reader can never observe `m` from round *t* beside `v`
    /// from round *t−1*.
    state: Mutex<Option<OptimizerState>>,
}

struct OptimizerState {
    /// `x_t` — the global model this aggregator last produced.
    global: Vec<f32>,
    /// First moment.
    m: Vec<f32>,
    /// Second moment.
    v: Vec<f32>,
}

impl FedOptAggregator {
    /// A FedOpt aggregator using the paper's unweighted-mean
    /// pseudo-gradient and the paper's defaults for `variant`.
    pub fn new(variant: FedOptVariant) -> Self {
        Self {
            variant,
            params: FedOptParams::for_variant(variant),
            base: None,
            state: Mutex::new(None),
        }
    }

    /// With explicit hyperparameters.
    pub fn with_params(variant: FedOptVariant, params: FedOptParams) -> Self {
        Self {
            variant,
            params,
            base: None,
            state: Mutex::new(None),
        }
    }

    /// Computes `Δ_t` from `base` instead of the unweighted mean.
    ///
    /// **A documented deviation from Reddi et al.**, not part of what the
    /// catalog names construct — see this type's fidelity notes. Useful
    /// for asking whether adaptive server optimization composes with
    /// Byzantine robustness, which is a real question and not one the
    /// paper answers.
    pub fn with_base(mut self, base: Box<dyn Aggregator>) -> Self {
        self.base = Some(base);
        self
    }

    /// The optimizer's current `(m, v)`, or `None` before the first
    /// round. Read-only, for tests and diagnostics; `aggregate` reads its
    /// own state directly.
    pub fn moments(&self) -> Option<(Vec<f32>, Vec<f32>)> {
        self.state
            .lock()
            .expect("FedOptAggregator state mutex poisoned")
            .as_ref()
            .map(|s| (s.m.clone(), s.v.clone()))
    }
}

impl Aggregator for FedOptAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        // The batch's aggregate, before any server optimization.
        let aggregate = match &self.base {
            Some(base) => base.aggregate(updates)?,
            None => {
                // Algorithm 2 line 10: the unweighted mean. `1/n` folded
                // into each term rather than applied after summing —
                // summing first overflows on large-but-finite weights.
                let share = 1.0 / decoded.len() as f32;
                let mut mean = vec![0.0f32; dim];
                for w in &decoded {
                    for (m, x) in mean.iter_mut().zip(w) {
                        *m += x * share;
                    }
                }
                mean
            }
        };

        let mut guard = self
            .state
            .lock()
            .expect("FedOptAggregator state mutex poisoned");

        let Some(state) = guard.as_mut() else {
            // Round one: this is `x_0`. There is no previous global model
            // to difference against, so there is no pseudo-gradient and
            // no optimizer step to take — the aggregate *is* the answer,
            // and it becomes `x_t` for round two.
            *guard = Some(OptimizerState {
                global: aggregate.clone(),
                m: vec![0.0; dim],
                // `v_{-1} = τ²`, the smallest Algorithm 2's
                // initialization condition (`v_{-1} ≥ τ²`) allows.
                v: vec![self.params.tau * self.params.tau; dim],
            });
            return Ok(aggregate);
        };

        // A model whose dimension changed mid-experiment has no
        // continuity with the accumulated moments — carrying them
        // forward would mean dividing this round's gradient by a moment
        // estimate belonging to a different parameter. Re-seed instead,
        // the same choice `CenteredClippingAggregator` makes.
        if state.global.len() != dim {
            *guard = Some(OptimizerState {
                global: aggregate.clone(),
                m: vec![0.0; dim],
                v: vec![self.params.tau * self.params.tau; dim],
            });
            return Ok(aggregate);
        }

        let FedOptParams {
            server_learning_rate: eta,
            beta1,
            beta2,
            tau,
        } = self.params;

        // `f64` for the accumulation, for the reason established
        // throughout this crate: a squared pseudo-gradient on a
        // large-but-finite update overflows `f32` long before the update
        // itself is unreasonable, and an infinite `v` makes every
        // subsequent step exactly zero — the optimizer would silently
        // stop learning rather than fail.
        let mut next = Vec::with_capacity(dim);
        for (i, (target, previous)) in aggregate.iter().zip(state.global.iter()).enumerate() {
            let delta = *target as f64 - *previous as f64;

            let m = beta1 as f64 * state.m[i] as f64 + (1.0 - beta1 as f64) * delta;

            let d2 = delta * delta;
            let v_prev = state.v[i] as f64;
            let v = match self.variant {
                FedOptVariant::Adagrad => v_prev + d2,
                FedOptVariant::Adam => beta2 as f64 * v_prev + (1.0 - beta2 as f64) * d2,
                FedOptVariant::Yogi => {
                    // `sign(v_{t-1} − Δ²)`: v shrinks only when the
                    // incoming squared gradient exceeds the running
                    // estimate.
                    let diff = v_prev - d2;
                    let sign = if diff > 0.0 {
                        1.0
                    } else if diff < 0.0 {
                        -1.0
                    } else {
                        0.0
                    };
                    v_prev - (1.0 - beta2 as f64) * d2 * sign
                }
            };
            // Yogi's subtraction can drive `v` negative on a large
            // incoming gradient, and `√negative` is `NaN`. Clamped at
            // the initialization floor `τ²`, which is the invariant
            // Algorithm 2's own analysis relies on ("v_{t-1,j} ≥ τ since
            // v_{-1} ≥ τ").
            let v = v.max((tau * tau) as f64);

            state.m[i] = m as f32;
            state.v[i] = v as f32;

            let step = eta as f64 * m / (v.sqrt() + tau as f64);
            next.push((*previous as f64 + step) as f32);
        }

        // A non-finite iterate would be checkpointed and become every
        // later round's starting point. Refuse instead — the same rule
        // every other aggregator here follows.
        if let Some(index) = next.iter().position(|w| !w.is_finite()) {
            return Err(AggregatorError::NonFiniteWeights {
                client_id: format!("<{} server optimizer>", self.variant.name()),
                index,
            });
        }

        state.global.copy_from_slice(&next);
        Ok(next)
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

    fn batch(weights: &[f32]) -> Vec<ClientDelta> {
        vec![
            delta("a", weights),
            delta("b", weights),
            delta("c", weights),
        ]
    }

    #[test]
    fn round_one_returns_the_plain_aggregate() {
        // Algorithm 2's `x_0`: no previous global, so no pseudo-gradient
        // and no optimizer step.
        for variant in [
            FedOptVariant::Adagrad,
            FedOptVariant::Adam,
            FedOptVariant::Yogi,
        ] {
            let agg = FedOptAggregator::new(variant);
            let out = agg.aggregate(&batch(&[1.0, 2.0, 3.0])).unwrap();
            assert_eq!(out, vec![1.0, 2.0, 3.0], "{variant:?}");
        }
    }

    #[test]
    fn the_optimizer_moves_the_model_in_the_pseudo_gradient_direction() {
        let agg = FedOptAggregator::new(FedOptVariant::Adam);
        // Round 1 establishes x_t = [0, 0, 0].
        agg.aggregate(&batch(&[0.0, 0.0, 0.0])).unwrap();
        // Round 2's batch points at [1, 1, 1], so Δ = [1, 1, 1].
        let out = agg.aggregate(&batch(&[1.0, 1.0, 1.0])).unwrap();

        assert!(
            out.iter().all(|w| *w > 0.0),
            "should move toward the aggregate, got {out:?}"
        );
    }

    #[test]
    fn adagrads_effective_step_only_ever_shrinks() {
        // `v_t = v_{t-1} + Δ²` accumulates without decay, so repeating an
        // identical pseudo-gradient must produce monotonically smaller
        // steps. This is the property that distinguishes Adagrad, and it
        // is checkable without knowing the exact numbers.
        let agg = FedOptAggregator::new(FedOptVariant::Adagrad);
        agg.aggregate(&batch(&[0.0])).unwrap();

        let mut previous = 0.0f32;
        let mut last_step = f32::INFINITY;
        for round in 0..5 {
            // Always ask for a full unit step from wherever we are.
            let target = previous + 1.0;
            let out = agg.aggregate(&batch(&[target])).unwrap();
            let step = out[0] - previous;
            assert!(
                step < last_step,
                "round {round}: step {step} did not shrink below {last_step}"
            );
            last_step = step;
            previous = out[0];
        }
    }

    #[test]
    fn yogis_second_moment_decays_more_slowly_than_adams() {
        // The structural difference between the two rules, read straight
        // off Algorithm 2 lines 13–14: Adam's `v` moves *multiplicatively*
        // (`β2·v + (1−β2)Δ²`), so it decays toward a new small-gradient
        // regime at 1% per round. Yogi's moves *additively* (`v −
        // (1−β2)Δ²` when `Δ² < v`), so it barely moves at all.
        //
        // The consequence is that Yogi is the more conservative of the
        // two after a shock: it holds `v` high, so its effective step
        // stays small, so a sudden large round cannot be followed by a
        // sudden large *step*. That is the property Yogi exists for.
        //
        // Worth recording that the first version of this test asserted
        // the opposite — that Yogi would recover *faster* — and failed.
        // The implementation was right and the assertion was backwards.
        let shock = [1000.0f32];
        let normal = [1.0f32];

        let mut second_moments = Vec::new();
        for variant in [FedOptVariant::Adam, FedOptVariant::Yogi] {
            let agg = FedOptAggregator::new(variant);
            agg.aggregate(&batch(&[0.0])).unwrap();
            agg.aggregate(&batch(&shock)).unwrap();
            // Several quiet rounds, during which Adam's `v` should decay
            // and Yogi's should not.
            for _ in 0..10 {
                agg.aggregate(&batch(&normal)).unwrap();
            }
            second_moments.push(agg.moments().unwrap().1[0]);
        }

        assert!(
            second_moments[1] > second_moments[0],
            "yogi's v should stay above adam's after a shock: yogi={} adam={}",
            second_moments[1],
            second_moments[0]
        );
    }

    #[test]
    fn the_variants_genuinely_differ() {
        // Three names that produced identical numbers would be three
        // aliases, not three methods.
        let mut results = Vec::new();
        for variant in [
            FedOptVariant::Adagrad,
            FedOptVariant::Adam,
            FedOptVariant::Yogi,
        ] {
            let agg = FedOptAggregator::new(variant);
            agg.aggregate(&batch(&[0.0, 0.0])).unwrap();
            agg.aggregate(&batch(&[1.0, 2.0])).unwrap();
            results.push(agg.aggregate(&batch(&[2.0, 4.0])).unwrap());
        }
        assert_ne!(results[0], results[1], "adagrad and adam agree");
        assert_ne!(results[1], results[2], "adam and yogi agree");
    }

    #[test]
    fn state_is_carried_across_rounds_not_recomputed() {
        let agg = FedOptAggregator::new(FedOptVariant::Adam);
        assert!(agg.moments().is_none(), "no moments before the first round");

        agg.aggregate(&batch(&[0.0, 0.0])).unwrap();
        let (m0, v0) = agg.moments().unwrap();
        assert_eq!(m0, vec![0.0, 0.0]);

        agg.aggregate(&batch(&[1.0, 1.0])).unwrap();
        let (m1, v1) = agg.moments().unwrap();
        assert_ne!(m0, m1, "the first moment should have moved");
        assert_ne!(v0, v1, "the second moment should have moved");
    }

    #[test]
    fn a_dimension_change_reseeds_rather_than_mixing_moments() {
        let agg = FedOptAggregator::new(FedOptVariant::Adam);
        agg.aggregate(&batch(&[1.0, 2.0])).unwrap();
        agg.aggregate(&batch(&[2.0, 3.0])).unwrap();

        let out = agg.aggregate(&batch(&[1.0, 2.0, 3.0])).unwrap();
        assert_eq!(out, vec![1.0, 2.0, 3.0], "re-seeded, not stepped");
        let (m, _) = agg.moments().unwrap();
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn extreme_but_finite_updates_do_not_produce_a_non_finite_model() {
        // The Tier 6 rule applied to the newest family: reject, or return
        // something finite. Never `NaN` into the checkpoint.
        for variant in [
            FedOptVariant::Adagrad,
            FedOptVariant::Adam,
            FedOptVariant::Yogi,
        ] {
            let agg = FedOptAggregator::new(variant);
            for w in [0.0, f32::MAX, -f32::MAX, 1.0, f32::MAX] {
                match agg.aggregate(&batch(&[w, w])) {
                    Ok(out) => assert!(
                        out.iter().all(|x| x.is_finite()),
                        "{variant:?} returned {out:?}"
                    ),
                    Err(_) => { /* refusing is a pass */ }
                }
            }
        }
    }

    #[test]
    fn wrapping_a_robust_base_uses_that_bases_pseudo_gradient() {
        // The documented extension: Δ_t comes from Krum rather than the
        // mean, so an outlier that would drag the mean does not.
        let robust = FedOptAggregator::new(FedOptVariant::Adam).with_base(
            crate::build_aggregator("krum", crate::AggregatorParams::default()).unwrap(),
        );
        let plain = FedOptAggregator::new(FedOptVariant::Adam);

        let poisoned = vec![
            delta("a", &[1.0, 1.0]),
            delta("b", &[1.1, 0.9]),
            delta("c", &[0.9, 1.1]),
            delta("attacker", &[500.0, 500.0]),
        ];

        let r1 = robust.aggregate(&poisoned).unwrap();
        let p1 = plain.aggregate(&poisoned).unwrap();

        assert!(
            r1[0] < 2.0,
            "krum-based Δ should ignore the attacker: {r1:?}"
        );
        assert!(p1[0] > 100.0, "the mean-based Δ should not: {p1:?}");
    }
}
