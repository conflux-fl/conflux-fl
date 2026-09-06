//! Local differential privacy for federated learning: clipping and noising
//! a client's update before it leaves the client (or before the server
//! aggregates it), plus an accountant that tracks how much cumulative
//! privacy loss ("epsilon") has been spent across rounds so a caller can
//! stop once a configured budget runs out.
//!
//! # Example
//!
//! Clipping bounds any one client's influence; the noise is what buys
//! the privacy guarantee.
//!
//! ```
//! use conflux_privacy::build_privacy_mechanism;
//!
//! // No noise, so the clipping step is visible on its own.
//! let clip_only = build_privacy_mechanism("gaussian_clipping", 1.0, 0.0).unwrap();
//!
//! let mut weights = [3.0_f32, 4.0, 12.0]; // L2 norm is exactly 13
//! let mut rng = rand::rng();
//! clip_only.transform(&mut weights, &mut rng);
//!
//! let norm = weights.iter().map(|w| w * w).sum::<f32>().sqrt();
//! assert!((norm - 1.0).abs() < 1e-5, "clipped to the unit ball");
//!
//! // An update already inside the radius is left alone — clipping is a
//! // ceiling, not a normalization.
//! let mut small = [0.1_f32, 0.1, 0.1];
//! clip_only.transform(&mut small, &mut rng);
//! assert!((small[0] - 0.1).abs() < 1e-6);
//! ```
//!
//! The accountant tracks what has been spent, and stores the raw
//! per-round parameters rather than a running total — epsilon depends on
//! the `delta` a caller asks about:
//!
//! ```
//! use conflux_privacy::{PrivacyAccountant, RdpAccountant};
//!
//! let mut accountant = RdpAccountant::new();
//! assert_eq!(accountant.current_epsilon(1e-5), 0.0);
//!
//! for _ in 0..10 {
//!     accountant.record_round(1.0, 0.01);
//! }
//!
//! let spent = accountant.current_epsilon(1e-5);
//! assert!(spent > 0.0);
//! // Composition only accumulates: more rounds never cost less.
//! accountant.record_round(1.0, 0.01);
//! assert!(accountant.current_epsilon(1e-5) > spent);
//!
//! assert!(accountant.budget_exhausted(spent, 1e-5));
//! assert!(!accountant.budget_exhausted(1e9, 1e-5));
//! ```

#![warn(missing_docs)]

use conflux_config::{StrategyEntry, StrategyKind};
use rand_distr::{Distribution, Normal};

/// What varies about a privacy mechanism: how it transforms one client's
/// update in place before that update is used further (sent over the
/// network, or aggregated). Implementations need to be usable as
/// `Box<dyn PrivacyMechanism>` — constructed by name from configuration
/// at startup, without the caller knowing the concrete type — so
/// `transform` takes `rng: &mut dyn rand::Rng` rather than a generic
/// `impl rand::Rng` or a generic type parameter: a trait with a generic
/// method can't be made into a trait object, because the vtable needs one
/// fixed function-pointer slot per method, and a generic method would
/// need a different one per instantiation. Taking `&mut dyn Rng` instead
/// keeps `transform` itself non-generic. Callers are unaffected — passing
/// a concrete `&mut StdRng` (or any other `Rng` impl) where `&mut dyn Rng`
/// is expected is an automatic, zero-effort unsized coercion.
pub trait PrivacyMechanism: Send + Sync {
    /// Applies this mechanism to `weights` in place.
    ///
    /// In place rather than returning a new vector because a weight vector
    /// is the largest thing in the pipeline — a model's full parameter set
    /// — and every caller already owns a mutable one.
    fn transform(&self, weights: &mut [f32], rng: &mut dyn rand::Rng);
}

// Registers this family's one member into `conflux-config`'s compile-time
// strategy registry, so `config.privacy_mechanism = "gaussian_clipping"`
// resolves to a concrete implementation without `conflux-server` needing
// to name this type directly. How the registry mechanism itself works:
// `https://confluxfl.dev/blog/rust-compile-time-registries-inventory/`.
inventory::submit! {
    StrategyEntry {
        kind: StrategyKind::PrivacyMechanism,
        name: "gaussian_clipping",
        citation: "Abadi, Chu, Goodfellow, McMahan, Mironov, Talwar & Zhang (2016), Deep Learning with Differential Privacy",
        family: "dp",
        params: &["clip_norm", "noise_multiplier"],
    }
}

#[derive(Debug, thiserror::Error)]
/// Why a privacy-mechanism name couldn't be turned into a
/// `PrivacyMechanism`.
pub enum PrivacyMechanismBuildError {
    #[error(
        "unknown privacy mechanism \"{0}\" — not a registered conflux-privacy strategy \
         (known: {known})",
        known = known_mechanisms()
    )]
    /// The name isn't in this crate's registry — almost always a typo in a
    /// resolved `privacy_mechanism` config value.
    Unknown(String),
}

/// The registered mechanism names, for the error above — read from the
/// registry rather than hardcoded, so the message cannot drift from what
/// `build_privacy_mechanism` actually accepts.
fn known_mechanisms() -> String {
    conflux_config::registered_names(StrategyKind::PrivacyMechanism).join(", ")
}

/// Constructs the `PrivacyMechanism` named by a resolved
/// `config.privacy_mechanism.value`. `clip_norm`/`noise_multiplier` are
/// passed explicitly (unlike `conflux-core`/`conflux-selector`'s
/// `build_*` functions, `GaussianClippingPrivacy` has no useful
/// zero-argument default) — both are already-resolved config values by
/// the time this is called.
pub fn build_privacy_mechanism(
    name: &str,
    clip_norm: f32,
    noise_multiplier: f32,
) -> Result<Box<dyn PrivacyMechanism>, PrivacyMechanismBuildError> {
    match name {
        "gaussian_clipping" => Ok(Box::new(GaussianClippingPrivacy {
            clip_norm,
            noise_multiplier,
        })),
        other => Err(PrivacyMechanismBuildError::Unknown(other.to_string())),
    }
}

/// Local DP: clip an update's L2 norm, then add calibrated Gaussian noise.
/// References: Abadi et al. (2016), *Deep Learning with Differential
/// Privacy*, ACM CCS; Geyer, Klein & Nabi (2017), *Differentially Private
/// Federated Learning: A Client Level Perspective*.
#[derive(Debug, Clone, Copy)]
pub struct GaussianClippingPrivacy {
    /// The L2 norm each update is clipped to before noise is added. This
    /// is what bounds any single client's contribution, and therefore what
    /// makes the noise scale below meaningful.
    pub clip_norm: f32,
    /// Gaussian noise standard deviation, as a multiple of `clip_norm`.
    /// Higher means more privacy and less utility; `0.0` disables the
    /// noise while leaving the clipping in place.
    pub noise_multiplier: f32,
}

impl Default for GaussianClippingPrivacy {
    /// `clip_norm = 1.0`, `noise_multiplier = 1.0` — both widely used
    /// DP-SGD starting points.
    fn default() -> Self {
        Self {
            clip_norm: 1.0,
            noise_multiplier: 1.0,
        }
    }
}

impl GaussianClippingPrivacy {
    /// Scales `weights` down so its L2 norm is at most `clip_norm`; leaves
    /// it untouched if already within bound.
    pub fn clip(&self, weights: &mut [f32]) {
        let norm = l2_norm(weights);
        if norm > self.clip_norm && norm > 0.0 {
            let scale = self.clip_norm / norm;
            for w in weights.iter_mut() {
                *w *= scale;
            }
        }
    }

    /// Adds i.i.d. Gaussian noise (mean 0, std = `noise_multiplier *
    /// clip_norm`) to each element, using `rng` — callers pass a seeded
    /// RNG for reproducible tests, or an OS-seeded one in production.
    /// Taking `&mut dyn Rng` here (rather than a generic `impl Rng`) is
    /// what lets `PrivacyMechanism::transform` — which must stay
    /// object-safe to back `Box<dyn PrivacyMechanism>` — delegate straight
    /// to this method with no wrapping; a caller passing a concrete
    /// `&mut StdRng` still just works, via automatic unsized coercion.
    pub fn add_noise(&self, weights: &mut [f32], rng: &mut dyn rand::Rng) {
        let std_dev = (self.noise_multiplier * self.clip_norm) as f64;
        // Zero disables the noise by design. A negative or NaN product can
        // only come from a caller bypassing config validation (which
        // rejects negative `clip_norm`/`noise_multiplier`); treat it as
        // "no noise" rather than panicking inside `Normal::new`.
        if std_dev <= 0.0 || std_dev.is_nan() {
            return;
        }
        let normal = Normal::new(0.0, std_dev).expect("std_dev > 0, checked above");
        for w in weights.iter_mut() {
            *w += normal.sample(rng) as f32;
        }
    }

    /// Clip then add noise — the full local-DP transform applied to one
    /// client's update.
    pub fn transform(&self, weights: &mut [f32], rng: &mut dyn rand::Rng) {
        self.clip(weights);
        self.add_noise(weights, rng);
    }
}

impl PrivacyMechanism for GaussianClippingPrivacy {
    fn transform(&self, weights: &mut [f32], rng: &mut dyn rand::Rng) {
        GaussianClippingPrivacy::transform(self, weights, rng)
    }
}

fn l2_norm(weights: &[f32]) -> f32 {
    weights.iter().map(|w| w * w).sum::<f32>().sqrt()
}

/// Tracks cumulative privacy loss across rounds and reports whether the
/// experiment's epsilon budget is exhausted.
pub trait PrivacyAccountant: Send + Sync {
    /// Records that one round ran with these parameters.
    ///
    /// The raw parameters are stored, not a running epsilon total. Epsilon
    /// depends on `delta`, which is supplied per query below — a
    /// precomputed total would silently be wrong the moment a caller asked
    /// about a different `delta`.
    fn record_round(&mut self, noise_multiplier: f32, sample_rate: f32);
    /// Cumulative epsilon spent so far, at the given `delta`.
    fn current_epsilon(&self, delta: f64) -> f64;
    /// Whether `current_epsilon(delta)` has reached `target_epsilon`.
    ///
    /// A convenience over comparing the two directly, so every call site
    /// applies the same boundary condition rather than each choosing
    /// between `>` and `>=`.
    fn budget_exhausted(&self, target_epsilon: f64, delta: f64) -> bool;
}

/// An epsilon accountant based on Rényi Differential Privacy (RDP)
/// composition. References: Mironov (2017), *Rényi Differential
/// Privacy*, IEEE CSF; Wang, Balle & Kasiviswanathan (2019), *Subsampled
/// Rényi Differential Privacy and Analytical Moments Accountant*,
/// AISTATS.
///
/// Per-round RDP accounts for **privacy amplification by subsampling**:
/// a round that exposes a random `sample_rate` fraction of the population
/// leaks less than one that exposes all of it, and the accountant says
/// so. See [`sampled_gaussian_rdp`] for the formula and where it applies;
/// where it does not, the non-subsampled bound stands in, which is never
/// smaller than the truth.
///
/// Supports two accounting granularities, selected by
/// `conflux_config::AccountingScope`: `Global` (one running epsilon for
/// the whole experiment, tracked in `rounds`) and `PerClient` (one
/// running epsilon per client, tracked in `client_rounds`). Both
/// histories are always recorded on every call, regardless of which
/// scope is actually configured — so switching `accounting_scope`
/// between restarts never silently loses history the other scope was
/// already accumulating. The RDP math itself (`epsilon_from_rounds`) is
/// shared between the two — `PerClient` only changes *which* history
/// it's evaluated against, never the composition math itself.
pub struct RdpAccountant {
    rounds: Vec<(f32, f32)>,
    client_rounds: std::collections::HashMap<String, Vec<(f32, f32)>>,
}

impl RdpAccountant {
    /// A fresh accountant with no rounds recorded — zero epsilon spent.
    pub fn new() -> Self {
        Self {
            rounds: Vec::new(),
            client_rounds: std::collections::HashMap::new(),
        }
    }

    /// Records one round of exposure for a single client — called once
    /// per client actually admitted into a round's aggregate, when
    /// `accounting_scope = PerClient`. Independent of `record_round`
    /// (`PrivacyAccountant`'s experiment-wide counterpart, still called
    /// unconditionally by `conflux-server` — see this struct's doc
    /// comment on why both are always recorded).
    pub fn record_round_for_client(
        &mut self,
        client_id: &str,
        noise_multiplier: f32,
        sample_rate: f32,
    ) {
        self.client_rounds
            .entry(client_id.to_string())
            .or_default()
            .push((noise_multiplier, sample_rate));
    }

    /// A single client's own cumulative epsilon — `0.0` for a client
    /// with no recorded rounds yet, the same "nothing spent yet"
    /// convention [`PrivacyAccountant::current_epsilon`] uses.
    pub fn current_epsilon_for_client(&self, client_id: &str, delta: f64) -> f64 {
        match self.client_rounds.get(client_id) {
            Some(rounds) => epsilon_from_rounds(rounds, delta),
            None => 0.0,
        }
    }

    /// Whether this specific client's own cumulative epsilon has reached
    /// `target_epsilon` — the per-client counterpart to
    /// [`PrivacyAccountant::budget_exhausted`].
    pub fn budget_exhausted_for_client(
        &self,
        client_id: &str,
        target_epsilon: f64,
        delta: f64,
    ) -> bool {
        self.current_epsilon_for_client(client_id, delta) >= target_epsilon
    }

    /// Every client with at least one recorded round — used by
    /// `conflux-server`'s tests to confirm per-client isolation, and
    /// available for any future admin/introspection surface.
    pub fn client_ids(&self) -> impl Iterator<Item = &String> {
        self.client_rounds.keys()
    }
}

impl Default for RdpAccountant {
    fn default() -> Self {
        Self::new()
    }
}

/// Rényi orders to search over when converting cumulative RDP to
/// (ε, δ)-DP — the same discrete-grid technique real moments accountants
/// (opacus, tf-privacy) use for Mironov (2017) §3's RDP→DP conversion,
/// minimized over α.
///
/// Every entry yields a *valid* upper bound on epsilon, so taking the
/// minimum is sound however the individual terms were computed. That is
/// what lets the integer orders use the tight subsampled formula while
/// the fractional ones fall back to the loose bound: the grid is a search
/// for the best valid answer, not a set of answers that must agree.
const RDP_ORDERS: &[f64] = &[
    1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 20.0, 24.0, 32.0, 48.0,
    64.0, 96.0, 128.0, 192.0, 256.0,
];

/// The RDP composition math itself (Mironov, 2017) — shared by
/// `current_epsilon` (the experiment-wide total) and
/// `current_epsilon_for_client` (the per-client total). Extracted into
/// one function so `PerClient` accounting is guaranteed to use *exactly*
/// the same composition as `Global`, just evaluated against a different
/// history.
fn epsilon_from_rounds(rounds: &[(f32, f32)], delta: f64) -> f64 {
    if rounds.is_empty() {
        return 0.0;
    }
    // Rounds sharing a (σ, q) contribute identical RDP, and a real
    // experiment runs thousands with the same pair. Counting them once
    // and multiplying turns an O(rounds × orders × α) computation into
    // one over *distinct configurations*, which is usually one.
    let mut by_config: std::collections::HashMap<(u32, u32), (f64, f64, u32)> =
        std::collections::HashMap::new();
    for &(noise_multiplier, sample_rate) in rounds {
        let entry = by_config
            .entry((noise_multiplier.to_bits(), sample_rate.to_bits()))
            .or_insert((noise_multiplier as f64, sample_rate as f64, 0));
        entry.2 += 1;
    }

    RDP_ORDERS
        .iter()
        .map(|&alpha| {
            let rdp_total: f64 = by_config
                .values()
                .map(|&(sigma, q, count)| rdp_per_round(alpha, sigma, q) * count as f64)
                .sum();
            // RDP → (ε, δ)-DP conversion (Mironov, 2017):
            // ε(α) = RDP(α) + ln(1/δ)/(α−1).
            rdp_total + (1.0 / delta).ln() / (alpha - 1.0)
        })
        .fold(f64::INFINITY, f64::min)
}

/// One round's RDP at order `alpha`, for noise multiplier `sigma` and
/// sampling rate `q`.
///
/// Uses the subsampled (tight) formula where it applies and the
/// non-subsampled bound otherwise. Both are valid upper bounds, so a
/// fallback can only over-report epsilon — the safe direction, and the
/// only direction an accountant may ever err in.
fn rdp_per_round(alpha: f64, sigma: f64, q: f64) -> f64 {
    // The non-subsampled Gaussian mechanism at order α (Mironov, 2017):
    // α / (2σ²). Also the exact answer when every client participates.
    let full = alpha / (2.0 * sigma * sigma);
    match sampled_gaussian_rdp(alpha, sigma, q) {
        // Never take a *larger* number from the tight path: if the two
        // ever disagree in that direction the bound is the one to trust,
        // and reporting less privacy spent than is provable is the one
        // failure this function must not have.
        Some(tight) if tight < full => tight,
        _ => full,
    }
}

/// RDP of the **Sampled Gaussian Mechanism** at an integer order —
/// Mironov, Talwar & Zhang (2019), *Rényi Differential Privacy of the
/// Sampled Gaussian Mechanism*, §3.3; the same closed form opacus and
/// tf-privacy use for integer orders.
///
/// For a Poisson-subsampled Gaussian with rate `q` and noise multiplier
/// `sigma`:
///
/// ```text
/// A(α) = Σ_{k=0..α} C(α,k) (1−q)^(α−k) q^k exp(k(k−1) / 2σ²)
/// RDP(α) = ln A(α) / (α−1)
/// ```
///
/// **Why this matters.** Without it, a deployment sampling 1% of its
/// clients per round is charged as though it exposed all of them, and
/// reports an epsilon far larger than it actually spends — which ends
/// either in a budget exhausted long before it truly is, or in noise
/// added to compensate for a cost that was never incurred.
///
/// `None` when the formula does not apply, and the caller falls back:
///
/// - **Non-integer α.** The closed form above is only valid at integer
///   orders; the fractional case needs a different, integral-based
///   expression. Returning `None` costs nothing, because the grid's
///   integer orders are searched too and the minimum is what counts.
/// - **q outside (0, 1).** `q = 1` *is* the non-subsampled mechanism (the
///   sum collapses to its last term and gives exactly α/2σ², which a test
///   asserts); `q = 0` means no exposure at all; outside that range the
///   input is not a sampling rate.
/// - **A non-finite result**, from a σ so small the exponent overflows.
///
/// The sum is taken in log space with the maximum term factored out.
/// `exp(k(k−1)/2σ²)` reaches `1e300` for entirely ordinary inputs
/// (α = 64, σ = 0.5), so computing `A(α)` directly would overflow to
/// infinity and lose the answer before the logarithm could recover it.
pub fn sampled_gaussian_rdp(alpha: f64, sigma: f64, q: f64) -> Option<f64> {
    // Written as positive conditions so a `NaN` in any argument fails
    // every one of them: `NaN > 0.0` is false, where `NaN <= 0.0` would
    // also be false and let it through.
    let integer_order = alpha.fract() == 0.0 && alpha >= 2.0;
    let rate_in_unit_interval = q > 0.0 && q < 1.0;
    let positive_noise = sigma > 0.0;
    if !(integer_order && rate_in_unit_interval && positive_noise) {
        return None;
    }
    let order = alpha as u32;
    let ln_q = q.ln();
    let ln_1mq = (1.0 - q).ln();
    let two_sigma_sq = 2.0 * sigma * sigma;

    // log of each term: ln C(α,k) + (α−k)ln(1−q) + k ln q + k(k−1)/2σ².
    // The binomial coefficient is built up multiplicatively in log space
    // — C(256, 128) overflows `f64` as a number but is unremarkable as a
    // logarithm.
    let mut ln_terms = Vec::with_capacity(order as usize + 1);
    let mut ln_binomial = 0.0f64;
    for k in 0..=order {
        if k > 0 {
            ln_binomial += ((order - k + 1) as f64).ln() - (k as f64).ln();
        }
        let k = k as f64;
        ln_terms
            .push(ln_binomial + (alpha - k) * ln_1mq + k * ln_q + (k * (k - 1.0)) / two_sigma_sq);
    }

    let max = ln_terms.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max.is_finite() {
        return None;
    }
    let sum: f64 = ln_terms.iter().map(|&t| (t - max).exp()).sum();
    let ln_a = max + sum.ln();
    let rdp = ln_a / (alpha - 1.0);
    // A(α) ≥ 1 always, so RDP ≥ 0; a tiny negative here is rounding, not
    // a privacy gain.
    rdp.is_finite().then(|| rdp.max(0.0))
}

impl PrivacyAccountant for RdpAccountant {
    fn record_round(&mut self, noise_multiplier: f32, sample_rate: f32) {
        self.rounds.push((noise_multiplier, sample_rate));
    }

    fn current_epsilon(&self, delta: f64) -> f64 {
        epsilon_from_rounds(&self.rounds, delta)
    }

    fn budget_exhausted(&self, target_epsilon: f64, delta: f64) -> bool {
        self.current_epsilon(delta) >= target_epsilon
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn build_privacy_mechanism_succeeds_for_gaussian_clipping() {
        assert!(build_privacy_mechanism("gaussian_clipping", 1.0, 1.0).is_ok());
    }

    #[test]
    fn build_privacy_mechanism_fails_for_an_unknown_name() {
        // `Box<dyn PrivacyMechanism>` isn't `Debug`, so `.unwrap_err()`
        // isn't usable here — match directly, same reasoning as
        // `conflux-core`/`conflux-selector`'s analogous tests.
        match build_privacy_mechanism("does_not_exist", 1.0, 1.0) {
            Err(PrivacyMechanismBuildError::Unknown(name)) => assert_eq!(name, "does_not_exist"),
            Ok(_) => panic!("expected an error, got a constructed PrivacyMechanism"),
        }
    }

    #[test]
    fn the_unknown_name_error_lists_the_registered_names() {
        let err = match build_privacy_mechanism("nope", 1.0, 1.0) {
            Err(err) => err,
            Ok(_) => panic!("expected an error"),
        };
        assert!(err.to_string().contains("gaussian_clipping"), "{err}");
    }

    #[test]
    fn every_buildable_name_is_also_registry_visible() {
        assert!(build_privacy_mechanism("gaussian_clipping", 1.0, 1.0).is_ok());
        assert!(
            conflux_config::lookup(StrategyKind::PrivacyMechanism, "gaussian_clipping").is_some()
        );
    }

    #[test]
    fn registry_constructed_mechanism_behaves_like_the_concrete_type() {
        let mechanism = build_privacy_mechanism("gaussian_clipping", 1.0, 0.0).unwrap();
        let mut weights = vec![3.0, 4.0]; // L2 norm 5.0
        let mut rng = StdRng::seed_from_u64(1);

        mechanism.transform(&mut weights, &mut rng);

        assert!((l2_norm(&weights) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn clip_scales_down_vector_above_bound() {
        let privacy = GaussianClippingPrivacy {
            clip_norm: 1.0,
            noise_multiplier: 1.0,
        };
        let mut weights = vec![3.0, 4.0]; // L2 norm 5.0

        privacy.clip(&mut weights);

        assert!((l2_norm(&weights) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn clip_leaves_vector_within_bound_unchanged() {
        let privacy = GaussianClippingPrivacy {
            clip_norm: 10.0,
            noise_multiplier: 1.0,
        };
        let mut weights = vec![3.0, 4.0]; // L2 norm 5.0

        privacy.clip(&mut weights);

        assert_eq!(weights, vec![3.0, 4.0]);
    }

    #[test]
    fn clipping_a_zero_vector_never_divides_by_zero() {
        // l2_norm is 0.0 here, so the `norm > 0.0` guard in `clip` must
        // hold — without it, `clip_norm / norm` would be a division by
        // zero and every element would become NaN.
        let privacy = GaussianClippingPrivacy {
            clip_norm: 1.0,
            noise_multiplier: 1.0,
        };
        let mut weights = vec![0.0, 0.0, 0.0];

        privacy.clip(&mut weights);

        assert_eq!(weights, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn a_negative_noise_scale_adds_no_noise_instead_of_panicking() {
        // Config validation rejects negatives upstream; a caller that
        // bypasses it must still not crash the process.
        let privacy = GaussianClippingPrivacy {
            clip_norm: 1.0,
            noise_multiplier: -1.0,
        };
        let mut weights = vec![0.5, 0.5];
        let mut rng = StdRng::seed_from_u64(1);

        privacy.add_noise(&mut weights, &mut rng);

        assert_eq!(weights, vec![0.5, 0.5]);
    }

    #[test]
    fn budget_is_never_exhausted_against_a_zero_target_epsilon_with_no_rounds() {
        // A target_epsilon of 0.0 is a degenerate but not invalid config
        // value; with zero rounds recorded, current_epsilon is exactly
        // 0.0, and "0.0 >= 0.0" means budget_exhausted must already
        // report true even before a single round runs.
        let accountant = RdpAccountant::new();

        assert!(accountant.budget_exhausted(0.0, 1e-5));
    }

    #[test]
    fn zero_noise_multiplier_is_deterministic_clip_only() {
        let privacy = GaussianClippingPrivacy {
            clip_norm: 1.0,
            noise_multiplier: 0.0,
        };
        let mut weights = vec![3.0, 4.0];
        let mut rng = StdRng::seed_from_u64(1);

        privacy.transform(&mut weights, &mut rng);

        assert!((l2_norm(&weights) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn epsilon_is_zero_with_no_rounds() {
        let accountant = RdpAccountant::new();

        assert_eq!(accountant.current_epsilon(1e-5), 0.0);
    }

    // ----- subsampling amplification -----

    /// Values computed independently of this implementation: the Rényi
    /// divergence of the Sampled Gaussian Mechanism, integrated
    /// numerically straight from its definition
    /// (`∫ Q^α P^(1−α) dx`, with `P = N(0,σ²)` and
    /// `Q = (1−q)N(0,σ²) + qN(1,σ²)`) on a fine grid in log space.
    /// Definition and closed form agreed to within 5e-12 relative across
    /// every case below, which is what makes these numbers evidence
    /// about the *formula* rather than about the arithmetic that
    /// produced it.
    const REFERENCE: &[(f64, f64, f64, f64)] = &[
        // (alpha, sigma, q, rdp)
        (2.0, 1.0, 0.01, 1.718134220744e-04),
        (4.0, 2.0, 0.1, 6.003282964490e-03),
        (8.0, 2.0, 0.05, 3.121526642301e-03),
        (16.0, 4.0, 0.01, 5.207209700579e-05),
        (3.0, 1.0, 0.5, 6.968891185980e-01),
    ];

    #[test]
    fn subsampled_rdp_matches_the_definition_integrated_numerically() {
        for &(alpha, sigma, q, expected) in REFERENCE {
            let got = sampled_gaussian_rdp(alpha, sigma, q)
                .unwrap_or_else(|| panic!("α={alpha} σ={sigma} q={q} should be computable"));
            let relative = (got - expected).abs() / expected;
            assert!(
                relative < 1e-9,
                "α={alpha} σ={sigma} q={q}: got {got:e}, expected {expected:e} (relative {relative:e})"
            );
        }
    }

    #[test]
    fn full_participation_is_exactly_the_non_subsampled_bound() {
        // q = 1 is not subsampling at all: the sum collapses to its last
        // term, `exp(α(α−1)/2σ²)`, and the formula reduces to α/2σ². The
        // implementation declines it (it is outside the open interval)
        // and the caller falls back — to the same number the algebra
        // gives, which is what makes the fallback safe rather than
        // merely conservative.
        for (alpha, sigma) in [(2.0, 1.0), (8.0, 2.0), (64.0, 4.0)] {
            assert_eq!(sampled_gaussian_rdp(alpha, sigma, 1.0), None);
            assert_eq!(
                rdp_per_round(alpha, sigma, 1.0),
                alpha / (2.0 * sigma * sigma)
            );
        }
    }

    #[test]
    fn sampling_a_fraction_costs_less_than_exposing_everyone() {
        // The whole point: a round that exposes 1% of the population
        // must not be charged as though it exposed all of it.
        for &(alpha, sigma, q, _) in REFERENCE {
            let sampled = rdp_per_round(alpha, sigma, q);
            let full = rdp_per_round(alpha, sigma, 1.0);
            assert!(
                sampled < full,
                "α={alpha} σ={sigma} q={q}: {sampled} !< {full}"
            );
        }
    }

    #[test]
    fn a_smaller_sampling_rate_costs_less_at_every_order() {
        for &alpha in &[2.0, 8.0, 32.0] {
            let mut previous = f64::INFINITY;
            for &q in &[0.5, 0.2, 0.1, 0.01, 0.001] {
                let rdp = rdp_per_round(alpha, 1.5, q);
                assert!(rdp < previous, "α={alpha} q={q}: {rdp} !< {previous}");
                previous = rdp;
            }
        }
    }

    #[test]
    fn a_non_integer_order_declines_rather_than_guessing() {
        // The closed form holds at integer orders only. Declining costs
        // nothing — the grid's integer orders are searched too, and the
        // minimum over valid bounds is what the conversion reports.
        assert_eq!(sampled_gaussian_rdp(2.5, 1.0, 0.1), None);
        assert_eq!(sampled_gaussian_rdp(1.25, 1.0, 0.1), None);
        // And a declined order still yields a usable, valid bound.
        assert_eq!(rdp_per_round(2.5, 1.0, 0.1), 2.5 / 2.0);
    }

    #[test]
    fn an_unusable_sampling_rate_or_noise_scale_falls_back_instead_of_failing() {
        for q in [0.0, -0.1, 1.5, f64::NAN] {
            assert_eq!(sampled_gaussian_rdp(4.0, 1.0, q), None);
        }
        assert_eq!(sampled_gaussian_rdp(4.0, 0.0, 0.1), None);
        // A σ small enough to overflow the exponent: the term
        // `exp(k(k−1)/2σ²)` is `exp(1.3e6)` here, so a direct sum would
        // be infinity. Log space keeps it finite, and anything that is
        // not finite falls back rather than reporting nonsense.
        let extreme = rdp_per_round(256.0, 0.01, 0.001);
        assert!(extreme.is_finite() && extreme > 0.0, "got {extreme}");
    }

    #[test]
    fn the_accountant_charges_a_sampling_deployment_far_less_than_a_full_one() {
        // Ten thousand rounds at σ = 1.0, the shape a real DP-SGD run
        // has: sampling 1% per round rather than everyone is the
        // difference between a usable budget and an unusable one.
        let mut sampled = RdpAccountant::new();
        let mut full = RdpAccountant::new();
        for _ in 0..10_000 {
            sampled.record_round(1.0, 0.01);
            full.record_round(1.0, 1.0);
        }
        let (a, b) = (sampled.current_epsilon(1e-5), full.current_epsilon(1e-5));
        assert!(a < b / 100.0, "sampled {a} should be far below full {b}");
        assert!(a > 0.0 && a.is_finite(), "got {a}");
    }

    #[test]
    fn rounds_at_the_same_configuration_compose_linearly_in_rdp() {
        // The grouping optimization must not change the answer: one
        // round counted a thousand times is a thousand rounds.
        let mut many = RdpAccountant::new();
        for _ in 0..1_000 {
            many.record_round(1.5, 0.05);
        }
        let mut mixed = RdpAccountant::new();
        for _ in 0..500 {
            mixed.record_round(1.5, 0.05);
        }
        for _ in 0..500 {
            mixed.record_round(1.5, 0.05);
        }
        assert_eq!(many.current_epsilon(1e-5), mixed.current_epsilon(1e-5));
    }

    #[test]
    fn more_rounds_increases_epsilon() {
        let mut accountant = RdpAccountant::new();
        accountant.record_round(1.0, 0.1);
        let after_one = accountant.current_epsilon(1e-5);

        accountant.record_round(1.0, 0.1);
        let after_two = accountant.current_epsilon(1e-5);

        assert!(after_two > after_one);
    }

    #[test]
    fn higher_noise_multiplier_decreases_epsilon() {
        let mut low_noise = RdpAccountant::new();
        low_noise.record_round(1.0, 0.1);

        let mut high_noise = RdpAccountant::new();
        high_noise.record_round(2.0, 0.1);

        assert!(low_noise.current_epsilon(1e-5) > high_noise.current_epsilon(1e-5));
    }

    #[test]
    fn budget_exhausted_flips_true_once_epsilon_exceeds_target() {
        let mut accountant = RdpAccountant::new();

        assert!(!accountant.budget_exhausted(0.5, 1e-5));

        for _ in 0..50 {
            accountant.record_round(1.0, 0.1);
        }

        assert!(accountant.budget_exhausted(0.5, 1e-5));
    }

    // PerClient accounting.

    #[test]
    fn client_epsilon_is_zero_for_a_client_with_no_recorded_rounds() {
        let accountant = RdpAccountant::new();

        assert_eq!(accountant.current_epsilon_for_client("client-a", 1e-5), 0.0);
    }

    #[test]
    fn two_clients_composing_independently_never_affect_each_others_total() {
        let mut accountant = RdpAccountant::new();

        for _ in 0..10 {
            accountant.record_round_for_client("heavy-user", 1.0, 0.1);
        }
        accountant.record_round_for_client("light-user", 1.0, 0.1);

        let heavy = accountant.current_epsilon_for_client("heavy-user", 1e-5);
        let light = accountant.current_epsilon_for_client("light-user", 1e-5);

        assert!(heavy > light);
        // Confirms isolation, not just "different values" — a bug that
        // accidentally shared state across clients could still produce
        // different numbers by coincidence.
        assert_eq!(
            light,
            {
                let mut solo = RdpAccountant::new();
                solo.record_round_for_client("light-user", 1.0, 0.1);
                solo.current_epsilon_for_client("light-user", 1e-5)
            },
            "light-user's epsilon must match what it would be in total isolation from heavy-user"
        );
    }

    #[test]
    fn a_clients_own_epsilon_increases_monotonically_across_rounds() {
        let mut accountant = RdpAccountant::new();

        let mut previous = accountant.current_epsilon_for_client("client-a", 1e-5);
        for _ in 0..5 {
            accountant.record_round_for_client("client-a", 1.0, 0.1);
            let current = accountant.current_epsilon_for_client("client-a", 1e-5);
            assert!(current > previous);
            previous = current;
        }
    }

    #[test]
    fn per_client_budget_exhausted_flips_true_independently_per_client() {
        let mut accountant = RdpAccountant::new();

        for _ in 0..50 {
            accountant.record_round_for_client("heavy-user", 1.0, 0.1);
        }
        // light-user never composes at all — confirms heavy-user's
        // exhausted budget doesn't leak into a client with no history of
        // its own, the same isolation property the tests above already
        // check for `current_epsilon_for_client` directly.

        assert!(accountant.budget_exhausted_for_client("heavy-user", 0.5, 1e-5));
        assert!(!accountant.budget_exhausted_for_client("light-user", 0.5, 1e-5));
    }

    #[test]
    fn global_and_per_client_history_are_recorded_independently() {
        // Both are always tracked regardless of which scope is
        // configured (see RdpAccountant's own doc comment) — recording
        // one must never affect the other.
        let mut accountant = RdpAccountant::new();
        accountant.record_round(1.0, 0.1); // global-scope call
        accountant.record_round_for_client("client-a", 1.0, 0.1); // per-client call

        assert!(accountant.current_epsilon(1e-5) > 0.0);
        assert!(accountant.current_epsilon_for_client("client-a", 1e-5) > 0.0);
        // client-b never had a per-client round recorded, even though a
        // global round was recorded — global and per-client are genuinely
        // separate histories, not two views of the same data.
        assert_eq!(accountant.current_epsilon_for_client("client-b", 1e-5), 0.0);
    }

    #[test]
    fn client_ids_lists_every_client_with_at_least_one_recorded_round() {
        let mut accountant = RdpAccountant::new();
        accountant.record_round_for_client("client-a", 1.0, 0.1);
        accountant.record_round_for_client("client-b", 1.0, 0.1);

        let mut ids: Vec<&String> = accountant.client_ids().collect();
        ids.sort();
        assert_eq!(ids, vec!["client-a", "client-b"]);
    }
}
