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
///   (`FoolsGoldAggregator`, `CenteredClippingAggregator`). A restarted
///   server resumes from its checkpoint with fresh moment estimates,
///   which costs a few rounds of adaptivity rather than correctness.
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

/// **FedAvgM** — Hsu, Qi & Brown, 2019, "Measuring the Effects of
/// Non-Identical Data Distribution for Federated Visual Classification".
///
/// FedAvg with a momentum buffer on the server, and nothing else:
///
/// ```text
/// v ← β v + Δw          the paper's own line, §4.2
/// w ← w − v
/// ```
///
/// It is the simplest member of this family and the one every adaptive
/// method is measured against — Reddi et al.'s own results table carries
/// a FedAvgM column, so a framework with FedOpt and without this cannot
/// reproduce it.
///
/// # Fidelity notes (ADR 0008)
///
/// - **`Δw` is `num_samples`-weighted**, per the paper's
///   `Δw = Σ (n_k/n) Δw_k`. This is the *opposite* choice from
///   [`FedOptAggregator`], whose Algorithm 2 specifies an unweighted
///   mean — the two papers genuinely disagree, and each is implemented
///   as written rather than harmonized.
/// - **Sign convention.** The paper writes `w ← w − v` because its `Δw`
///   is a descent direction. Conflux's clients return trained *weights*,
///   so the natural quantity here is `Δ = aggregate − x_t`, an ascent
///   direction, and the update is `x ← x + v`. Identical arithmetic,
///   opposite sign convention; nothing about the method changes.
/// - **`η = 1.0` is the paper's own value** here, not a placeholder:
///   "The learning rate of the server optimizer is held constant at
///   1.0." That is the one honest default in this whole family.
/// - **Classical momentum, not Nesterov.** The paper's §4.2 equation is
///   classical (`v ← βv + Δw`), while its experiments say Nesterov
///   accelerated gradient. The two disagree inside the paper; this
///   implements the equation as written, and says so rather than
///   picking silently.
/// - **State does not survive a restart**, like every stateful method
///   here.
pub struct FedAvgMAggregator {
    /// Momentum coefficient `β`. The paper sweeps
    /// `{0, 0.7, 0.9, 0.97, 0.99, 0.997}` and reports 0.9 working well
    /// broadly, which is the default.
    pub beta: f32,
    /// Server learning rate. `1.0` per the paper.
    pub server_learning_rate: f32,
    /// `None` means the `num_samples`-weighted mean the paper
    /// specifies — i.e. plain FedAvg. `Some` swaps in another base,
    /// which is a documented extension rather than the paper.
    base: Option<Box<dyn Aggregator>>,
    /// `(x_t, v)` — ADR 0012's pattern, as everywhere else here.
    state: Mutex<Option<(Vec<f32>, Vec<f32>)>>,
}

impl Default for FedAvgMAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl FedAvgMAggregator {
    /// FedAvgM with the paper's `β = 0.9` and `η = 1.0`.
    pub fn new() -> Self {
        Self {
            beta: 0.9,
            server_learning_rate: 1.0,
            base: None,
            state: Mutex::new(None),
        }
    }

    /// Computes `Δw` from `base` rather than the paper's weighted mean.
    /// A documented deviation, same terms as
    /// [`FedOptAggregator::with_base`].
    pub fn with_base(mut self, base: Box<dyn Aggregator>) -> Self {
        self.base = Some(base);
        self
    }

    /// The current momentum buffer, or `None` before the first round.
    /// Read-only, for tests and diagnostics.
    pub fn momentum(&self) -> Option<Vec<f32>> {
        self.state
            .lock()
            .expect("FedAvgMAggregator state mutex poisoned")
            .as_ref()
            .map(|(_, v)| v.clone())
    }
}

impl Aggregator for FedAvgMAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let aggregate = match &self.base {
            Some(base) => base.aggregate(updates)?,
            // The paper's `Σ (n_k/n) Δw_k` — which is exactly FedAvg.
            None => crate::FedAvg::default().aggregate(updates)?,
        };

        let mut guard = self
            .state
            .lock()
            .expect("FedAvgMAggregator state mutex poisoned");

        let reseed = match guard.as_ref() {
            None => true,
            Some((global, _)) => global.len() != dim,
        };
        if reseed {
            // Round one is `x_0`: no previous global, so no `Δw` and no
            // momentum step. Same as Algorithm 2's initialization.
            *guard = Some((aggregate.clone(), vec![0.0; dim]));
            return Ok(aggregate);
        }

        let (global, velocity) = guard.as_mut().expect("checked above");
        let beta = self.beta as f64;
        let eta = self.server_learning_rate as f64;

        let mut next = Vec::with_capacity(dim);
        for ((target, previous), v) in aggregate.iter().zip(global.iter()).zip(velocity.iter_mut())
        {
            // `f64` for the same reason as everywhere else in this crate:
            // a momentum buffer accumulates, so an `f32` overflow here
            // would be permanent rather than momentary.
            let delta = *target as f64 - *previous as f64;
            let updated = beta * *v as f64 + delta;
            *v = updated as f32;
            next.push((*previous as f64 + eta * updated) as f32);
        }

        if let Some(index) = next.iter().position(|w| !w.is_finite()) {
            return Err(AggregatorError::NonFiniteWeights {
                client_id: "<fedavgm server optimizer>".to_string(),
                index,
            });
        }

        global.copy_from_slice(&next);
        Ok(next)
    }
}

/// **q-FedAvg** — Li, Sanjabi, Beirami & Smith, 2020, "Fair Resource
/// Allocation in Federated Learning" (ICLR), Algorithm 2.
///
/// The only fairness-oriented method in the catalog. Every other method
/// here optimizes the *mean* — q-FedAvg optimizes the accuracy
/// *distribution*, by weighting each client by its own loss raised to a
/// power `q`, so clients the model currently serves badly pull harder:
///
/// ```text
/// Δw_k = L(w_t − w̄_k)                                local update, scaled by L
/// Δ_k  = F_k^q(w_t) · Δw_k                            weighted by loss^q
/// h_k  = q·F_k^{q−1}(w_t)·‖Δw_k‖² + L·F_k^q(w_t)
/// w_{t+1} = w_t − (Σ Δ_k) / (Σ h_k)
/// ```
///
/// `q = 0` recovers FedAvg exactly. Larger `q` trades mean accuracy for
/// a more uniform one — the paper's whole point is that this is a dial,
/// not a free improvement.
///
/// # This needs something no other method here needs
///
/// `F_k(w_t)` is **the client's own local loss at the round's starting
/// model**, which is not derivable from the update. It arrives as
/// `ClientDelta::local_loss`, the third optional field ADR 0012's
/// mechanism carries. A client that does not report one is treated as
/// having no opinion and falls back to `num_samples` weighting for that
/// round — an unreported loss must not be read as a loss of zero, which
/// `q > 0` would turn into zero weight.
///
/// # Fidelity notes (ADR 0008)
///
/// - **Sign convention.** The paper's `Δw_k = L(w_t − w̄_k)` is a descent
///   direction, and it subtracts. Conflux's clients return trained
///   weights, so this works with `d_k = w̄_k − w_t` and adds. Identical
///   arithmetic.
/// - **`L` is not derived.** The paper estimates the Lipschitz constant
///   once by grid search at `q = 0` and reuses it across `q` (its Lemma
///   3). Conflux cannot do that estimation — it never sees a loss
///   surface — so `L` is config-supplied (`server_lipschitz`, builtin
///   `1.0`) and is a placeholder in the same sense `clip_radius` is.
///   It is the inverse of the client learning rate in the paper's own
///   framing.
/// - **`q` is the method.** Builtin `0.0`, which is exactly FedAvg. That
///   is deliberate: selecting `qfedavg` without choosing a `q` should
///   behave like the thing it generalizes rather than silently applying
///   a fairness trade nobody asked for.
/// - **Self-reported loss is trusted, and the direction matters.** Every
///   other unauthenticated field here can only be *inflated* to gain
///   influence; this one is the same, but more directly — q-FedAvg
///   weights *up* whoever claims a high loss. That is the published
///   method's own assumption, and it is why this is not a robustness
///   method and must not be read as one.
pub struct QFedAvgAggregator {
    /// The fairness exponent. `0.0` is FedAvg.
    pub q: f32,
    /// The Lipschitz estimate `L`. See the fidelity notes.
    pub lipschitz: f32,
    /// `w_t`, tracked as this aggregator's own previous output — the
    /// same approach [`FedOptAggregator`] uses, and for the same reason.
    state: Mutex<Option<Vec<f32>>>,
}

impl Default for QFedAvgAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl QFedAvgAggregator {
    /// q-FedAvg with `q = 0` (i.e. FedAvg) and `L = 1`.
    pub fn new() -> Self {
        Self {
            q: 0.0,
            lipschitz: 1.0,
            state: Mutex::new(None),
        }
    }

    /// With an explicit fairness exponent and Lipschitz estimate.
    pub fn with_params(q: f32, lipschitz: f32) -> Self {
        Self {
            q,
            lipschitz,
            state: Mutex::new(None),
        }
    }
}

impl Aggregator for QFedAvgAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let mut guard = self
            .state
            .lock()
            .expect("QFedAvgAggregator state mutex poisoned");

        let reseed = match guard.as_ref() {
            None => true,
            Some(w) => w.len() != dim,
        };
        if reseed {
            // Round one: no `w_t` to difference against. The paper's
            // Algorithm 2 needs one, so this round is the plain
            // sample-count mean, which also establishes `w_t`.
            let seed = crate::FedAvg::default().aggregate(updates)?;
            *guard = Some(seed.clone());
            return Ok(seed);
        }
        let global = guard.as_mut().expect("checked above");

        // Every client must report a loss for the method to mean
        // anything. If none does, this is FedAvg wearing a different
        // name, and saying so beats pretending otherwise.
        let any_loss = updates.iter().any(|u| u.local_loss.is_some());
        if !any_loss {
            let out = crate::FedAvg::default().aggregate(updates)?;
            global.copy_from_slice(&out);
            return Ok(out);
        }

        let q = self.q as f64;
        let l = self.lipschitz as f64;

        let mut numerator = vec![0.0f64; dim];
        let mut h_sum = 0.0f64;

        for (u, w) in updates.iter().zip(&decoded) {
            // A client that reported nothing is given the batch's
            // neutral loss of 1.0, so `F^q = 1` and it is weighted
            // exactly as FedAvg would weight it — no opinion, no
            // penalty.
            let loss = u.local_loss.map_or(1.0f64, |f| f as f64).max(0.0);

            // d_k = w̄_k − w_t (ascent); the paper's Δw_k = L(w_t − w̄_k)
            // is −L·d_k.
            let mut sq = 0.0f64;
            for (x, prev) in w.iter().zip(global.iter()) {
                let d = l * (*x as f64 - *prev as f64);
                sq += d * d;
            }

            let f_q = loss.powf(q);
            let f_q_minus_1 = if q == 0.0 { 0.0 } else { loss.powf(q - 1.0) };

            // h_k = q·F^{q−1}·‖Δw_k‖² + L·F^q
            let h = q * f_q_minus_1 * sq + l * f_q;
            if !h.is_finite() {
                continue;
            }
            h_sum += h;

            // Σ Δ_k, with the sign flipped into Conflux's ascent
            // convention so the final step adds rather than subtracts.
            for (acc, (x, prev)) in numerator.iter_mut().zip(w.iter().zip(global.iter())) {
                *acc += f_q * l * (*x as f64 - *prev as f64);
            }
        }

        if h_sum <= 0.0 || !h_sum.is_finite() {
            // Nothing usable — every client's weight collapsed. Falling
            // back to the plain mean beats returning the model unchanged
            // and silently stalling the experiment.
            let out = crate::FedAvg::default().aggregate(updates)?;
            global.copy_from_slice(&out);
            return Ok(out);
        }

        let next: Vec<f32> = global
            .iter()
            .zip(&numerator)
            .map(|(w, acc)| (*w as f64 + acc / h_sum) as f32)
            .collect();

        if let Some(index) = next.iter().position(|w| !w.is_finite()) {
            return Err(AggregatorError::NonFiniteWeights {
                client_id: "<qfedavg server optimizer>".to_string(),
                index,
            });
        }

        global.copy_from_slice(&next);
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

    fn delta_n(client_id: &str, weights: &[f32], num_samples: u64) -> ClientDelta {
        ClientDelta {
            client_id: client_id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples,
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
    fn fedavgm_accumulates_momentum_across_rounds() {
        // The property the method exists for: a consistent pull in one
        // direction compounds, so the same Δ produces a *larger* step
        // each round. Classical momentum, `v ← βv + Δ`.
        let agg = FedAvgMAggregator::new();
        agg.aggregate(&batch(&[0.0])).unwrap();

        let mut previous = 0.0f32;
        let mut last_step = 0.0f32;
        for round in 0..4 {
            let target = previous + 1.0;
            let out = agg.aggregate(&batch(&[target])).unwrap();
            let step = out[0] - previous;
            assert!(
                step > last_step,
                "round {round}: momentum should compound, {step} !> {last_step}"
            );
            last_step = step;
            previous = out[0];
        }
    }

    #[test]
    fn fedavgm_with_beta_zero_is_plain_fedavg() {
        // `v ← 0·v + Δ` is just `Δ`, so `x ← x + Δ` returns the
        // aggregate itself. A useful identity: it means the momentum is
        // the only thing this adds, and β = 0 is in the paper's own
        // sweep.
        let mut agg = FedAvgMAggregator::new();
        agg.beta = 0.0;
        agg.aggregate(&batch(&[0.0, 0.0])).unwrap();
        let out = agg.aggregate(&batch(&[1.0, 2.0])).unwrap();
        assert!(
            (out[0] - 1.0).abs() < 1e-6 && (out[1] - 2.0).abs() < 1e-6,
            "got {out:?}, expected the plain aggregate"
        );
    }

    #[test]
    fn fedavgm_weights_by_sample_count_unlike_fedopt() {
        // The two papers genuinely disagree, and each is implemented as
        // written. FedAvgM's Δw is `Σ (n_k/n) Δw_k`; FedOpt's Algorithm 2
        // line 10 is an unweighted mean. A client with 10x the samples
        // should therefore move FedAvgM more than it moves FedAdam.
        let lopsided = vec![delta_n("small", &[0.0], 1), delta_n("large", &[10.0], 100)];

        let m = FedAvgMAggregator::new();
        let a = FedOptAggregator::new(FedOptVariant::Adam);
        let m1 = m.aggregate(&lopsided).unwrap()[0];
        let a1 = a.aggregate(&lopsided).unwrap()[0];

        assert!(
            m1 > a1,
            "sample-count weighting should favour the large client: \
             fedavgm={m1} fedadam={a1}"
        );
        assert!((m1 - 9.9).abs() < 0.2, "weighted mean ~9.9, got {m1}");
        assert!((a1 - 5.0).abs() < 0.2, "unweighted mean 5.0, got {a1}");
    }

    #[test]
    fn fedavgm_survives_extreme_but_finite_updates() {
        let agg = FedAvgMAggregator::new();
        for w in [0.0, f32::MAX, -f32::MAX, 1.0] {
            match agg.aggregate(&batch(&[w, w])) {
                Ok(out) => assert!(out.iter().all(|x| x.is_finite()), "got {out:?}"),
                Err(_) => { /* refusing is a pass */ }
            }
        }
    }

    fn delta_loss(client_id: &str, weights: &[f32], loss: f32) -> ClientDelta {
        ClientDelta {
            client_id: client_id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples: 10,
            local_loss: Some(loss),
            ..Default::default()
        }
    }

    #[test]
    fn qfedavg_with_q_zero_matches_fedavg() {
        // The identity the paper states: q = 0 recovers FedAvg. If this
        // drifts, the "fairness dial" framing is false — there would be
        // no setting at which the method is the thing it generalizes.
        let q0 = QFedAvgAggregator::with_params(0.0, 1.0);
        let plain = crate::FedAvg::default();

        let round1 = vec![
            delta_loss("a", &[0.0, 0.0], 1.0),
            delta_loss("b", &[0.0, 0.0], 1.0),
        ];
        q0.aggregate(&round1).unwrap();

        let round2 = vec![
            delta_loss("a", &[1.0, 2.0], 0.5),
            delta_loss("b", &[3.0, 4.0], 9.0),
        ];
        let got = q0.aggregate(&round2).unwrap();
        let want = plain.aggregate(&round2).unwrap();

        for (g, w) in got.iter().zip(&want) {
            assert!(
                (g - w).abs() < 1e-4,
                "q=0 must equal fedavg: {got:?} vs {want:?}"
            );
        }
    }

    #[test]
    fn a_higher_loss_client_pulls_harder_when_q_is_positive() {
        // The method's entire purpose. Two clients pulling in opposite
        // directions, identical in every way except reported loss: with
        // q > 0 the aggregate must land nearer the one the model is
        // serving badly.
        let agg = QFedAvgAggregator::with_params(2.0, 1.0);
        agg.aggregate(&[delta_loss("a", &[0.0], 1.0), delta_loss("b", &[0.0], 1.0)])
            .unwrap();

        let out = agg
            .aggregate(&[
                delta_loss("well-served", &[-1.0], 0.1),
                delta_loss("badly-served", &[1.0], 5.0),
            ])
            .unwrap();

        assert!(
            out[0] > 0.0,
            "the high-loss client should dominate, got {out:?}"
        );
    }

    #[test]
    fn larger_q_shifts_the_direction_further_toward_the_worst_served_client() {
        // q is a dial, not a switch: the *direction* should move
        // monotonically toward the high-loss client as q grows.
        //
        // Direction, specifically — not the landing position. An earlier
        // version of this test asserted on `out[0]` directly and failed
        // at q = 4, and the implementation was right. `h_k` carries a
        // `q·F^{q−1}·‖Δw_k‖²` term that grows with q, so the denominator
        // grows and the *step shrinks* even as the direction keeps
        // turning. That coupling is the paper's own Lipschitz-based step
        // bound doing its job, not a defect: q-FedAvg becomes both more
        // fairness-weighted and more cautious at once.
        //
        // Isolating direction from step size: with tiny deltas the
        // quadratic term vanishes, `h_k → L·F^q`, and the step reduces
        // to the loss-weighted mean direction `Σ F^q d_k / Σ F^q`, which
        // *is* monotone in q.
        let epsilon = 1e-3;
        let mut previous = f32::NEG_INFINITY;
        for q in [0.0f32, 1.0, 2.0, 4.0] {
            let agg = QFedAvgAggregator::with_params(q, 1.0);
            agg.aggregate(&[delta_loss("a", &[0.0], 1.0), delta_loss("b", &[0.0], 1.0)])
                .unwrap();
            let out = agg
                .aggregate(&[
                    delta_loss("well-served", &[-epsilon], 0.5),
                    delta_loss("badly-served", &[epsilon], 4.0),
                ])
                .unwrap();
            // Normalized so the comparison is about direction alone.
            let direction = out[0] / epsilon;
            assert!(
                direction > previous,
                "q={q}: direction should turn further toward the high-loss \
                 client, {direction} !> {previous}"
            );
            previous = direction;
        }
        assert!(
            previous > 0.5,
            "at q = 4 the direction should be dominated by the high-loss \
             client, got {previous}"
        );
    }

    #[test]
    fn the_step_magnitude_is_non_monotone_in_q() {
        // The other half of what the direction test discovered, and it
        // is stranger than "bigger q, smaller step". Two effects compete:
        //
        //   direction — `F^q` weighting turns the step toward the
        //               high-loss client, which *increases* how far the
        //               aggregate lands from the origin;
        //   step size — `h_k`'s `q·F^{q−1}·‖Δw_k‖²` term grows with q,
        //               which *decreases* it.
        //
        // Neither dominates throughout, so the magnitude rises and then
        // falls. Measured here: 0.538 at q=1, 0.624 at q=2, 0.499 at
        // q=4.
        //
        // This is recorded because two successive versions of these
        // tests assumed monotonicity — first upward, then downward — and
        // both failed against a correct implementation. q is not a
        // simple "more fairness" dial: it trades mean accuracy,
        // uniformity, *and* convergence speed simultaneously, and the
        // net effect on any one of them is not monotone.
        let step = |q: f32| {
            let agg = QFedAvgAggregator::with_params(q, 1.0);
            agg.aggregate(&[delta_loss("a", &[0.0], 1.0), delta_loss("b", &[0.0], 1.0)])
                .unwrap();
            agg.aggregate(&[
                delta_loss("well-served", &[-1.0], 0.5),
                delta_loss("badly-served", &[1.0], 4.0),
            ])
            .unwrap()[0]
                .abs()
        };

        let (s1, s2, s4) = (step(1.0), step(2.0), step(4.0));
        assert!(
            s2 > s1,
            "the direction effect should win first: {s2} !> {s1}"
        );
        assert!(
            s4 < s2,
            "and the step-size penalty should win eventually: {s4} !< {s2}"
        );
    }

    #[test]
    fn a_client_reporting_no_loss_is_neutral_not_zero() {
        // `local_loss` is optional, and `q > 0` would turn a missing
        // value read as 0.0 into *zero weight* — silently excluding
        // every client that has not been upgraded to report it.
        let agg = QFedAvgAggregator::with_params(2.0, 1.0);
        agg.aggregate(&[delta_loss("a", &[0.0], 1.0), delta_loss("b", &[0.0], 1.0)])
            .unwrap();

        let mut silent = delta_loss("silent", &[1.0], 0.0);
        silent.local_loss = None;
        let out = agg
            .aggregate(&[silent, delta_loss("vocal", &[-1.0], 1.0)])
            .unwrap();

        assert!(
            out[0].abs() < 0.9,
            "a silent client must still count, got {out:?}"
        );
    }

    #[test]
    fn a_batch_with_no_reported_losses_is_plain_fedavg() {
        // No loss anywhere means the method has nothing to work with.
        // Behaving as FedAvg is honest; inventing weights is not.
        let agg = QFedAvgAggregator::with_params(2.0, 1.0);
        agg.aggregate(&batch(&[0.0, 0.0])).unwrap();
        let out = agg.aggregate(&batch(&[1.0, 3.0])).unwrap();
        let want = crate::FedAvg::default()
            .aggregate(&batch(&[1.0, 3.0]))
            .unwrap();
        assert_eq!(out, want);
    }

    #[test]
    fn qfedavg_survives_extreme_but_finite_updates() {
        let agg = QFedAvgAggregator::with_params(2.0, 1.0);
        for (w, loss) in [(0.0, 1.0), (f32::MAX, 1e30), (-f32::MAX, 0.0), (1.0, 1.0)] {
            match agg.aggregate(&[
                delta_loss("a", &[w, w], loss),
                delta_loss("b", &[1.0, 1.0], 1.0),
            ]) {
                Ok(out) => assert!(out.iter().all(|x| x.is_finite()), "got {out:?}"),
                Err(_) => { /* refusing is a pass */ }
            }
        }
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

/// The largest local step count this method will honour.
///
/// `local_steps` is self-reported and unauthenticated, exactly like
/// `num_samples`, and FedNova divides by it — so it is the *small*
/// values that are dangerous, not the large ones. A client reporting
/// `τ_k = 1` while everyone else reports 1000 buys a thousandfold
/// amplification of its own update. The ceiling here does not fix that
/// (nothing on the server can, without authenticating the count); it
/// bounds `τ_eff`, which a single client claiming `u32::MAX` would
/// otherwise drag to absurdity for the whole batch.
///
/// See [`crate::MAX_PLAUSIBLE_SAMPLE_COUNT`] for the same reasoning
/// applied to the other self-reported scalar.
pub const MAX_PLAUSIBLE_LOCAL_STEPS: u32 = 1 << 20;

/// **FedNova** — Wang, Liu, Liang, Joshi & Poor (2020), *Tackling the
/// Objective Inconsistency Problem in Heterogeneous Federated
/// Optimization*, NeurIPS.
///
/// # The problem it solves
///
/// FedAvg averages clients' *final weights*. When clients take different
/// numbers of local steps — because they have different amounts of data,
/// or different hardware, or simply drop out early — the client that
/// took more steps has travelled further, and averaging the endpoints
/// silently weights it more. The optimum FedAvg converges to is then not
/// the optimum of the objective anyone wrote down. That is the
/// "objective inconsistency" of the title, and it is a bias, not noise:
/// more rounds do not remove it.
///
/// FedNova normalizes each client's *progress* by the number of steps it
/// took, then rescales by the batch's effective step count:
///
/// ```text
/// x_{t+1} = x_t − τ_eff · Σ_k p_k · (x_t − x_k) / τ_k
///     p_k = n_k / Σn                (the FedAvg weight)
///     τ_eff = Σ_k p_k τ_k           (weighted mean step count)
/// ```
///
/// # Why this is stateful, contradicting two of this project's own docs
///
/// `AGGREGATION_LANDSCAPE.md` and ADR 0012 both record FedNova as
/// "fits `AveragingWeighting` cleanly" — a per-client reweighting needing
/// only the new `local_steps` field. **That is wrong**, and it is worth
/// writing down why, because the error is not obvious.
///
/// Expanding the update above and collecting the `x_t` terms gives
///
/// ```text
/// x_{t+1} = x_t · (1 − τ_eff·S) + τ_eff · Σ_k (p_k/τ_k) · x_k,
///     where S = Σ_k p_k/τ_k
/// ```
///
/// which is a weighted average of the clients' weights *only* if
/// `τ_eff·S = 1`. By Cauchy–Schwarz, `(Σ p_k τ_k)(Σ p_k/τ_k) ≥ 1` with
/// equality **iff every `τ_k` is equal** — which is precisely the case
/// where FedNova degenerates to FedAvg and does nothing. So the one
/// regime where the `AveragingWeighting` formulation would be correct is
/// the regime where the method has no effect; everywhere it matters, the
/// `x_t` term is real and must be carried across rounds.
///
/// It therefore follows ADR 0012's `Mutex` pattern like the rest of this
/// module.
///
/// # Fidelity notes
///
/// - **`x_t` is this aggregator's own previous output**, the same
///   approach [`FedOptAggregator`] and [`QFedAvgAggregator`] take, and
///   for the same reason: the server's checkpoint is what it last
///   emitted.
/// - **A client reporting no `local_steps` is given the batch's mean
///   `τ`**, so it is weighted exactly as FedAvg would weight it — no
///   opinion, no penalty. A batch where *nobody* reports one falls back
///   to FedAvg outright, because with all `τ_k` equal that is what
///   FedNova reduces to anyway.
/// - **`τ_k = 0` is treated as absent, not as zero.** The formula
///   divides by `τ_k`; a client honestly reporting that it took no steps
///   has no normalized direction to contribute, and one dishonestly
///   reporting it would otherwise produce an infinity.
pub struct FedNovaAggregator {
    /// `x_t`. ADR 0012's pattern.
    state: Mutex<Option<Vec<f32>>>,
}

impl Default for FedNovaAggregator {
    fn default() -> Self {
        Self::new()
    }
}

impl FedNovaAggregator {
    /// A fresh aggregator with no `x_t` yet — round one seeds it.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(None),
        }
    }
}

/// One client's `(p_k, τ_k)`, already validated.
struct NovaTerm {
    /// `n_k / Σn`, in `f64` — see the module's overflow notes.
    p: f64,
    /// `τ_k`, clamped to [`MAX_PLAUSIBLE_LOCAL_STEPS`] and never zero.
    tau: f64,
}

impl Aggregator for FedNovaAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let mut guard = self
            .state
            .lock()
            .expect("FedNovaAggregator state mutex poisoned");

        let reseed = match guard.as_ref() {
            None => true,
            // A dimension change means a different model; the stored
            // `x_t` describes something else and differencing against it
            // would be meaningless.
            Some(x) => x.len() != dim,
        };
        if reseed {
            let seed = crate::FedAvg::default().aggregate(updates)?;
            *guard = Some(seed.clone());
            return Ok(seed);
        }

        // `τ_k = 0` is not a step count, it is a missing one — see the
        // type docs.
        let reported: Vec<Option<u32>> = updates
            .iter()
            .map(|u| u.local_steps.filter(|t| *t > 0))
            .collect();
        if reported.iter().all(Option::is_none) {
            // Nobody is running FedNova. With every `τ_k` equal (to
            // whatever we would substitute), FedNova *is* FedAvg, so
            // this returns the same answer by a shorter route rather
            // than pretending to have done something else.
            let out = crate::FedAvg::default().aggregate(updates)?;
            guard.as_mut().expect("checked above").copy_from_slice(&out);
            return Ok(out);
        }

        // The mean of what *was* reported, substituted for what was not.
        let reported_sum: f64 = reported.iter().flatten().map(|t| *t as f64).sum();
        let reported_count = reported.iter().flatten().count() as f64;
        let default_tau = (reported_sum / reported_count).max(1.0);

        // `p_k` normalized here rather than after accumulating: summing
        // raw `num_samples * weight` products first is the overflow that
        // Tier 6 found in four separate places.
        let total_samples: f64 = updates.iter().map(|u| u.num_samples as f64).sum();
        let equal_share = 1.0 / updates.len() as f64;

        let terms: Vec<NovaTerm> = updates
            .iter()
            .zip(&reported)
            .map(|(u, tau)| NovaTerm {
                // A batch reporting no samples at all is degenerate but
                // legal; weighting it uniformly beats dividing by zero.
                p: if total_samples > 0.0 {
                    u.num_samples as f64 / total_samples
                } else {
                    equal_share
                },
                tau: tau
                    .map(|t| t.min(MAX_PLAUSIBLE_LOCAL_STEPS) as f64)
                    .unwrap_or(default_tau),
            })
            .collect();

        let tau_eff: f64 = terms.iter().map(|t| t.p * t.tau).sum();
        if !tau_eff.is_finite() || tau_eff <= 0.0 {
            let out = crate::FedAvg::default().aggregate(updates)?;
            guard.as_mut().expect("checked above").copy_from_slice(&out);
            return Ok(out);
        }

        let global = guard.as_mut().expect("checked above");

        // x_{t+1} = x_t + τ_eff · Σ_k (p_k/τ_k) · (x_k − x_t)
        //
        // Written as an ascent on `(x_k − x_t)` rather than a descent on
        // `(x_t − x_k)` to match this crate's sign convention, and
        // accumulated in `f64`: `x_k − x_t` between two finite `f32`s can
        // reach `2·f32::MAX`, which overflows `f32` before the scaling
        // that would have brought it back.
        let mut delta = vec![0.0f64; dim];
        for (term, w) in terms.iter().zip(&decoded) {
            let scale = tau_eff * term.p / term.tau;
            if !scale.is_finite() {
                continue;
            }
            for (acc, (x, prev)) in delta.iter_mut().zip(w.iter().zip(global.iter())) {
                *acc += scale * (*x as f64 - *prev as f64);
            }
        }

        let next: Vec<f32> = global
            .iter()
            .zip(&delta)
            .map(|(x, d)| (*x as f64 + d) as f32)
            .collect();

        if let Some(index) = next.iter().position(|w| !w.is_finite()) {
            return Err(AggregatorError::NonFiniteWeights {
                client_id: "<fednova server optimizer>".to_string(),
                index,
            });
        }

        global.copy_from_slice(&next);
        Ok(next)
    }
}

/// **SCAFFOLD** — Karimireddy, Kale, Mohri, Reddi, Stich & Suresh (2020),
/// *SCAFFOLD: Stochastic Controlled Averaging for Federated Learning*,
/// ICML.
///
/// # The problem it solves
///
/// On non-IID data each client's local optimum sits somewhere different
/// from the global one, so local training *drifts* toward it. Averaging
/// drifted models gives a global model biased toward whoever drifted
/// furthest — and unlike a variance problem, more clients per round does
/// not fix it. SCAFFOLD corrects each client's gradient by
/// `(c − c_i)`: the difference between the server's estimate of the
/// global update direction and this client's own. The correction cancels
/// the client's drift without either side needing the other's data.
///
/// # The half that did not exist before this method
///
/// SCAFFOLD is the first method in Conflux whose algorithm requires the
/// server to send *state* down to clients, not just weights. ADR 0012
/// added `ClientDelta.control_variate`, carrying each client's `Δc_i`
/// **up**; there was no corresponding field going down, so `c` could be
/// maintained and never delivered — and a client that never learns `c`
/// cannot compute `(c − c_i)`. `TaskResponse.control_variate` and the
/// [`Aggregator::control_variate`] hook are that missing half.
///
/// # The update (Algorithm 1, option II)
///
/// ```text
/// x ← x + η_g · (1/|S|) · Σ_{i∈S} Δy_i
/// c ← c + (1/N)  · Σ_{i∈S} Δc_i
/// ```
///
/// Note the two denominators differ, and deliberately so: the model moves
/// by the mean over *participating* clients, while the control variate is
/// damped by the total client population `N`, because it is an estimate
/// of a global quantity that only `|S|` clients contributed evidence
/// about this round. Using `|S|` for both would let `c` swing with
/// whoever happened to be sampled.
///
/// # Fidelity notes
///
/// - **`Δy_i` is derived, not received.** Conflux transmits full weight
///   vectors rather than deltas (ADR 0004), so `Δy_i = y_i − x_t` is
///   computed here against this aggregator's own previous output — the
///   same approach [`FedOptAggregator`], [`QFedAvgAggregator`] and
///   [`FedNovaAggregator`] take.
/// - **`Δc_i` is received**, in `ClientDelta.control_variate`, and is
///   length-checked against the model. The transport cannot check it
///   (ADR 0004: it is opaque to model architecture), so this is the
///   first place that can.
/// - **`num_clients` is the `N` above.** It cannot be inferred from a
///   batch, which only ever shows `|S|`. Defaulting it to the batch size
///   would silently turn the `1/N` damping into `1/|S|` and change the
///   method.
/// - **A client that sends no `Δc_i` still contributes its `Δy_i`.**
///   Excluding it would discard real training; contributing nothing to
///   `c` is the honest reading of "this client is not running SCAFFOLD".
pub struct ScaffoldAggregator {
    /// `η_g`, the global step size. `1.0` is the paper's default and
    /// makes the model update the plain mean of the `Δy_i`.
    pub server_learning_rate: f32,
    /// `N` — the total client population, not this round's sample.
    pub num_clients: u32,
    /// `(x_t, c)`. ADR 0012's pattern, as everywhere else in this module.
    state: Mutex<Option<(Vec<f32>, Vec<f32>)>>,
}

impl Default for ScaffoldAggregator {
    fn default() -> Self {
        Self::new(1.0, 1)
    }
}

impl ScaffoldAggregator {
    /// `server_learning_rate` is `η_g`; `num_clients` is `N`.
    ///
    /// `num_clients` is clamped to at least 1 — a zero would divide the
    /// control-variate update by nothing.
    pub fn new(server_learning_rate: f32, num_clients: u32) -> Self {
        Self {
            server_learning_rate,
            num_clients: num_clients.max(1),
            state: Mutex::new(None),
        }
    }
}

impl ScaffoldAggregator {
    /// `(1/N)·Σ Δc_i` over whoever sent a usable variate this round.
    ///
    /// Shared by the seed round and every later one, deliberately: the
    /// defect this refactor closed was the seed branch *not* running it
    /// — see the seed-round test for what that silently cost.
    fn variate_step(&self, updates: &[ClientDelta], dim: usize) -> Vec<f64> {
        let damping = 1.0 / self.num_clients as f64;
        let mut step = vec![0.0f64; dim];
        for update in updates {
            let Some(raw) = update.control_variate.as_ref() else {
                continue;
            };
            let Ok(dc) = conflux_proto::decode_weights(raw) else {
                // Undecodable is a malformed submission, not a zero
                // correction. Skipped rather than guessed at.
                continue;
            };
            // The length check ADR 0012 says a reader of this field must
            // perform: a short or long variate is not interpretable
            // against this model, and zero-padding it would manufacture
            // a correction the client never sent.
            if dc.len() != dim || dc.iter().any(|v| !v.is_finite()) {
                continue;
            }
            for (acc, v) in step.iter_mut().zip(&dc) {
                *acc += damping * *v as f64;
            }
        }
        step
    }
}

impl Aggregator for ScaffoldAggregator {
    fn aggregate(&self, updates: &[ClientDelta]) -> Result<Vec<f32>, AggregatorError> {
        if updates.is_empty() {
            return Err(AggregatorError::EmptyBatch);
        }
        let decoded = decode_and_validate(updates)?;
        let dim = decoded[0].len();

        let mut guard = self
            .state
            .lock()
            .expect("ScaffoldAggregator state mutex poisoned");

        let reseed = match guard.as_ref() {
            None => true,
            Some((x, _)) => x.len() != dim,
        };
        if reseed {
            // Round one: no `x_t` to difference against, so there is no
            // `Δy_i` and no optimizer step — the plain mean is the
            // answer. The batch's `Δc_i` are NOT skipped along with it:
            // by the time this runs, every client has already folded its
            // matching `c_i⁺` into its own state, and SCAFFOLD's
            // correction is unbiased only while `c = mean(c_i)`.
            // Discarding them here broke that invariant permanently —
            // measured as a constant 0.1277 bias on a quadratic where
            // the method is provably exact, and as monotonically
            // climbing held-out loss on MNIST.
            let seed = crate::FedAvg::default().aggregate(updates)?;
            let step = self.variate_step(updates, dim);
            let c: Vec<f32> = step.iter().map(|d| *d as f32).collect();
            let c = if c.iter().all(|v| v.is_finite()) {
                c
            } else {
                vec![0.0; dim]
            };
            *guard = Some((seed.clone(), c));
            return Ok(seed);
        }

        let (global, control) = guard.as_mut().expect("checked above");

        // x ← x + η_g · mean_i(Δy_i), with 1/|S| folded into each term
        // rather than applied to a total that has already overflowed —
        // the Tier 6 lesson, applied here rather than rediscovered.
        let share = 1.0 / decoded.len() as f64;
        let lr = self.server_learning_rate as f64;
        let mut model_step = vec![0.0f64; dim];
        for w in &decoded {
            for (acc, (y, x)) in model_step.iter_mut().zip(w.iter().zip(global.iter())) {
                // f64: two finite f32s can be 2·f32::MAX apart, which
                // overflows f32 before any scaling brings it back.
                *acc += share * (*y as f64 - *x as f64);
            }
        }

        // c ← c + (1/N) · Σ Δc_i, over whoever actually sent one.
        let variate_step = self.variate_step(updates, dim);
        let contributors = variate_step.iter().any(|d| *d != 0.0) as usize;

        let next: Vec<f32> = global
            .iter()
            .zip(&model_step)
            .map(|(x, d)| (*x as f64 + lr * d) as f32)
            .collect();

        if let Some(index) = next.iter().position(|w| !w.is_finite()) {
            return Err(AggregatorError::NonFiniteWeights {
                client_id: "<scaffold server optimizer>".to_string(),
                index,
            });
        }

        // `c` is only advanced once the model update is known good.
        // Writing a state that a later validation then rejects is
        // exactly how `centered_clipping` was left permanently `NaN`.
        if contributors > 0 {
            let next_control: Vec<f32> = control
                .iter()
                .zip(&variate_step)
                .map(|(c, d)| (*c as f64 + d) as f32)
                .collect();
            if next_control.iter().all(|v| v.is_finite()) {
                control.copy_from_slice(&next_control);
            }
        }

        global.copy_from_slice(&next);
        Ok(next)
    }

    /// Hands `c` to the round pipeline for dispatch. This is the half of
    /// SCAFFOLD that no other method needs.
    fn control_variate(&self) -> Option<Vec<f32>> {
        self.state
            .lock()
            .expect("ScaffoldAggregator state mutex poisoned")
            .as_ref()
            .map(|(_, c)| c.clone())
    }
}

#[cfg(test)]
mod fednova_tests {
    use super::*;
    use conflux_proto::encode_weights;

    fn delta_ns(id: &str, weights: &[f32], num_samples: u64, steps: Option<u32>) -> ClientDelta {
        ClientDelta {
            client_id: id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples,
            local_steps: steps,
            ..Default::default()
        }
    }

    /// Round one has no `x_t` to difference against, so it is FedAvg and
    /// establishes the state — the same contract `FedOpt` and `q-FedAvg`
    /// have.
    #[test]
    fn the_first_round_is_fedavg_and_seeds_the_state() {
        let agg = FedNovaAggregator::new();
        let batch = vec![
            delta_ns("a", &[0.0, 0.0], 10, Some(5)),
            delta_ns("b", &[2.0, 4.0], 10, Some(50)),
        ];
        let out = agg.aggregate(&batch).unwrap();
        assert_eq!(out, vec![1.0, 2.0], "equal samples -> plain mean");
    }

    /// The property the method exists for. Two clients, equal data, but
    /// one trained 10x longer. FedAvg averages the endpoints and lets the
    /// long-running client pull the model most of the way to itself;
    /// FedNova normalizes by step count first.
    #[test]
    fn a_client_that_trained_longer_does_not_get_extra_pull() {
        let seed = vec![
            delta_ns("a", &[0.0], 10, Some(1)),
            delta_ns("b", &[0.0], 10, Some(1)),
        ];

        let nova = FedNovaAggregator::new();
        nova.aggregate(&seed).unwrap(); // x_t = [0.0]

        // `a` took 1 step and moved 1.0; `b` took 10 steps and moved 10.0.
        // Per *step* they made identical progress.
        let batch = vec![
            delta_ns("a", &[1.0], 10, Some(1)),
            delta_ns("b", &[10.0], 10, Some(10)),
        ];

        let fedavg = crate::FedAvg::default().aggregate(&batch).unwrap();
        let out = nova.aggregate(&batch).unwrap();

        // FedAvg lands at the midpoint of the endpoints, 5.5 — dominated
        // by whoever ran longer.
        assert!((fedavg[0] - 5.5).abs() < 1e-5, "fedavg = {}", fedavg[0]);

        // FedNova: p = 0.5 each, tau_eff = 0.5*1 + 0.5*10 = 5.5.
        // step = 5.5 * (0.5/1 * 1.0 + 0.5/10 * 10.0) = 5.5 * 1.0 = 5.5
        // Both clients contribute the *same* normalized direction (1.0
        // per step), which is the whole point — neither dominates.
        assert!((out[0] - 5.5).abs() < 1e-4, "fednova = {}", out[0]);

        // Now the case that separates them: `b` runs 10x longer but makes
        // the *same total* progress, i.e. far less per step.
        let nova2 = FedNovaAggregator::new();
        nova2.aggregate(&seed).unwrap();
        let batch2 = vec![
            delta_ns("a", &[1.0], 10, Some(1)),
            delta_ns("b", &[1.0], 10, Some(10)),
        ];
        let fedavg2 = crate::FedAvg::default().aggregate(&batch2).unwrap();
        let out2 = nova2.aggregate(&batch2).unwrap();

        assert!((fedavg2[0] - 1.0).abs() < 1e-5, "fedavg2 = {}", fedavg2[0]);
        // tau_eff = 5.5; step = 5.5 * (0.5*1.0 + 0.05*1.0) = 3.025
        assert!(
            (out2[0] - 3.025).abs() < 1e-3,
            "fednova2 = {} — must differ from fedavg's 1.0",
            out2[0]
        );
    }

    /// With every client on the same step count FedNova *is* FedAvg —
    /// the degenerate case, and the one where the discarded
    /// `AveragingWeighting` formulation would have been correct.
    #[test]
    fn equal_step_counts_reduce_exactly_to_fedavg() {
        let seed = vec![
            delta_ns("a", &[0.0], 10, Some(7)),
            delta_ns("b", &[0.0], 10, Some(7)),
        ];
        let agg = FedNovaAggregator::new();
        agg.aggregate(&seed).unwrap();

        let batch = vec![
            delta_ns("a", &[1.0, -2.0], 10, Some(7)),
            delta_ns("b", &[3.0, 6.0], 30, Some(7)),
        ];
        let expected = crate::FedAvg::default().aggregate(&batch).unwrap();
        let out = agg.aggregate(&batch).unwrap();

        for (o, e) in out.iter().zip(&expected) {
            assert!((o - e).abs() < 1e-4, "got {o}, fedavg says {e}");
        }
    }

    /// A batch where nobody reports a step count must not silently
    /// invent one — it falls back to FedAvg, which is what FedNova
    /// reduces to when every tau is equal anyway.
    #[test]
    fn a_batch_reporting_no_steps_falls_back_to_fedavg() {
        let seed = vec![
            delta_ns("a", &[0.0], 10, None),
            delta_ns("b", &[0.0], 10, None),
        ];
        let agg = FedNovaAggregator::new();
        agg.aggregate(&seed).unwrap();

        let batch = vec![
            delta_ns("a", &[1.0], 10, None),
            delta_ns("b", &[5.0], 30, None),
        ];
        let expected = crate::FedAvg::default().aggregate(&batch).unwrap();
        assert_eq!(agg.aggregate(&batch).unwrap(), expected);
    }

    /// `tau = 0` is a missing step count, not a zero one. The formula
    /// divides by tau; treating a reported zero literally produces an
    /// infinity from a finite, validation-passing submission — the exact
    /// failure shape Tier 6 found four times elsewhere.
    #[test]
    fn a_zero_step_count_does_not_divide_by_zero() {
        let seed = vec![
            delta_ns("a", &[0.0], 10, Some(4)),
            delta_ns("b", &[0.0], 10, Some(4)),
        ];
        let agg = FedNovaAggregator::new();
        agg.aggregate(&seed).unwrap();

        let batch = vec![
            delta_ns("a", &[1.0], 10, Some(0)),
            delta_ns("b", &[2.0], 10, Some(4)),
        ];
        let out = agg.aggregate(&batch).unwrap();
        assert!(out[0].is_finite(), "got {}", out[0]);
    }

    /// An absurd step count must not drag `tau_eff` — and therefore the
    /// whole batch's step size — to infinity.
    #[test]
    fn an_absurd_step_count_is_clamped_rather_than_believed() {
        let seed = vec![
            delta_ns("a", &[0.0], 10, Some(4)),
            delta_ns("b", &[0.0], 10, Some(4)),
        ];
        let agg = FedNovaAggregator::new();
        agg.aggregate(&seed).unwrap();

        let batch = vec![
            delta_ns("a", &[1.0], 10, Some(u32::MAX)),
            delta_ns("b", &[1.0], 10, Some(4)),
        ];
        let out = agg.aggregate(&batch).unwrap();
        assert!(out[0].is_finite(), "got {}", out[0]);
    }

    /// Cross-round: extreme-but-finite weights must not poison `x_t` for
    /// every later round, which is how `centered_clipping` was broken.
    #[test]
    fn an_extreme_finite_update_does_not_permanently_poison_the_state() {
        let agg = FedNovaAggregator::new();
        agg.aggregate(&[
            delta_ns("a", &[0.0], 10, Some(2)),
            delta_ns("b", &[0.0], 10, Some(2)),
        ])
        .unwrap();

        let hostile = vec![
            delta_ns("a", &[f32::MAX], 10, Some(1)),
            delta_ns("b", &[-f32::MAX], 10, Some(1000)),
        ];
        // May legitimately reject; must not panic, and must not store
        // a non-finite state.
        let _ = agg.aggregate(&hostile);

        let honest = vec![
            delta_ns("a", &[1.0], 10, Some(5)),
            delta_ns("b", &[1.0], 10, Some(5)),
        ];
        let out = agg.aggregate(&honest).unwrap();
        assert!(
            out.iter().all(|w| w.is_finite()),
            "state was permanently poisoned: {out:?}"
        );
    }
}

#[cfg(test)]
mod scaffold_tests {
    use super::*;
    use conflux_proto::encode_weights;

    fn delta_cv(id: &str, weights: &[f32], variate: Option<&[f32]>) -> ClientDelta {
        ClientDelta {
            client_id: id.to_string(),
            round: 1,
            weights: encode_weights(weights),
            num_samples: 10,
            control_variate: variate.map(encode_weights),
            ..Default::default()
        }
    }

    /// Round one seeds `x_t` and starts `c` at zero — the paper's own
    /// initialization, and the one that makes the correction a no-op
    /// until any evidence exists.
    #[test]
    fn the_first_round_seeds_the_state_and_a_zero_control_variate() {
        let agg = ScaffoldAggregator::new(1.0, 10);
        let out = agg
            .aggregate(&[
                delta_cv("a", &[0.0, 2.0], None),
                delta_cv("b", &[2.0, 4.0], None),
            ])
            .unwrap();
        assert_eq!(out, vec![1.0, 3.0]);
        assert_eq!(agg.control_variate(), Some(vec![0.0, 0.0]));
    }

    /// The seed round must fold the batch's control variates into `c`
    /// rather than discard them, because the clients have already folded
    /// the matching `c_i⁺` into their own state by the time this runs.
    ///
    /// SCAFFOLD's correction is unbiased only while `c = mean(c_i)` —
    /// an invariant the paired updates preserve exactly under full
    /// participation. A server that drops round one's deltas breaks it
    /// *permanently*: every later round's correction carries the
    /// constant `mean(c_i) − c` bias. Measured, not hypothesized — on a
    /// deterministic quadratic (where SCAFFOLD is provably exact) the
    /// discard left the model converged 0.1277 from the optimum, the
    /// bias to four decimals; on MNIST it showed up as held-out loss
    /// climbing monotonically while accuracy plateaued.
    #[test]
    fn the_seed_round_folds_control_variates_instead_of_discarding_them() {
        let agg = ScaffoldAggregator::new(1.0, 2);
        agg.aggregate(&[
            delta_cv("a", &[0.0], Some(&[4.0])),
            delta_cv("b", &[0.0], Some(&[2.0])),
        ])
        .unwrap();

        // c = 0 + (1/2)(4.0 + 2.0) = 3.0. The discard behavior returns
        // Some([0.0]) here — a c the clients' own state contradicts.
        assert_eq!(
            agg.control_variate(),
            Some(vec![3.0]),
            "round one's deltas are evidence the clients already acted on"
        );
    }

    /// The half that did not exist before: the server has to be able to
    /// hand `c` back out. Without this the correction is uncomputable
    /// client-side, however correct the server's own arithmetic is.
    #[test]
    fn the_control_variate_is_exposed_for_dispatch_and_damped_by_n_not_by_batch_size() {
        // N = 4, batch = 2. Damping must use 4.
        let agg = ScaffoldAggregator::new(1.0, 4);
        agg.aggregate(&[delta_cv("a", &[0.0], None), delta_cv("b", &[0.0], None)])
            .unwrap();

        agg.aggregate(&[
            delta_cv("a", &[1.0], Some(&[8.0])),
            delta_cv("b", &[1.0], Some(&[4.0])),
        ])
        .unwrap();

        // c = 0 + (1/4)(8) + (1/4)(4) = 3.0.  Had it used |S| = 2 it
        // would be 6.0 — the silent method change the type docs warn of.
        let c = agg.control_variate().expect("scaffold maintains one");
        assert!((c[0] - 3.0).abs() < 1e-5, "c = {}, expected 3.0", c[0]);
    }

    /// Every other method must leave the wire field absent, so adding
    /// SCAFFOLD changed nothing for the eighteen that ignore it.
    #[test]
    fn other_aggregators_expose_no_control_variate() {
        assert_eq!(crate::FedAvg::default().control_variate(), None);
        assert_eq!(FedNovaAggregator::new().control_variate(), None);
        assert_eq!(FedAvgMAggregator::new().control_variate(), None);
    }

    /// The length check ADR 0012 says a reader of this field must do.
    /// The transport cannot: it is opaque to model architecture.
    #[test]
    fn a_wrong_length_control_variate_is_ignored_rather_than_padded() {
        let agg = ScaffoldAggregator::new(1.0, 2);
        agg.aggregate(&[
            delta_cv("a", &[0.0, 0.0], None),
            delta_cv("b", &[0.0, 0.0], None),
        ])
        .unwrap();

        // Two-weight model, one-element variate.
        agg.aggregate(&[
            delta_cv("a", &[1.0, 1.0], Some(&[5.0])),
            delta_cv("b", &[1.0, 1.0], None),
        ])
        .unwrap();

        assert_eq!(
            agg.control_variate(),
            Some(vec![0.0, 0.0]),
            "a mismatched variate must contribute nothing, not be zero-padded"
        );
    }

    /// A non-finite variate must not reach the stored state — the
    /// `centered_clipping` lesson, applied to the other piece of state
    /// this method keeps.
    #[test]
    fn a_non_finite_control_variate_cannot_poison_c() {
        let agg = ScaffoldAggregator::new(1.0, 2);
        agg.aggregate(&[delta_cv("a", &[0.0], None), delta_cv("b", &[0.0], None)])
            .unwrap();

        agg.aggregate(&[
            delta_cv("a", &[1.0], Some(&[f32::NAN])),
            delta_cv("b", &[1.0], Some(&[2.0])),
        ])
        .unwrap();

        let c = agg.control_variate().unwrap();
        assert!(c[0].is_finite(), "c = {}", c[0]);
        // Only `b`'s finite 2.0 counted: 0 + (1/2)(2.0) = 1.0
        assert!((c[0] - 1.0).abs() < 1e-5, "c = {}", c[0]);
    }

    /// A client that sends no variate still contributes its model
    /// update — excluding it would discard real training.
    #[test]
    fn a_client_without_a_variate_still_contributes_its_weights() {
        let agg = ScaffoldAggregator::new(1.0, 2);
        agg.aggregate(&[delta_cv("a", &[0.0], None), delta_cv("b", &[0.0], None)])
            .unwrap();

        let out = agg
            .aggregate(&[
                delta_cv("a", &[4.0], None),
                delta_cv("b", &[2.0], Some(&[1.0])),
            ])
            .unwrap();
        // x = 0 + 1.0 * mean(4-0, 2-0) = 3.0
        assert!((out[0] - 3.0).abs() < 1e-5, "x = {}", out[0]);
    }

    /// The server learning rate scales the model step and nothing else.
    #[test]
    fn the_server_learning_rate_scales_the_model_step() {
        let agg = ScaffoldAggregator::new(0.5, 2);
        agg.aggregate(&[delta_cv("a", &[0.0], None), delta_cv("b", &[0.0], None)])
            .unwrap();
        let out = agg
            .aggregate(&[delta_cv("a", &[4.0], None), delta_cv("b", &[4.0], None)])
            .unwrap();
        assert!(
            (out[0] - 2.0).abs() < 1e-5,
            "x = {}, expected 0.5*4",
            out[0]
        );
    }

    /// Cross-round: an extreme-but-finite batch must not leave the
    /// aggregator permanently unusable.
    #[test]
    fn an_extreme_finite_update_does_not_permanently_poison_the_state() {
        let agg = ScaffoldAggregator::new(1.0, 2);
        agg.aggregate(&[delta_cv("a", &[0.0], None), delta_cv("b", &[0.0], None)])
            .unwrap();

        let _ = agg.aggregate(&[
            delta_cv("a", &[f32::MAX], Some(&[f32::MAX])),
            delta_cv("b", &[-f32::MAX], Some(&[-f32::MAX])),
        ]);

        let out = agg
            .aggregate(&[delta_cv("a", &[1.0], None), delta_cv("b", &[1.0], None)])
            .unwrap();
        assert!(out.iter().all(|w| w.is_finite()), "poisoned: {out:?}");
        assert!(
            agg.control_variate().unwrap().iter().all(|v| v.is_finite()),
            "c poisoned"
        );
    }
}
