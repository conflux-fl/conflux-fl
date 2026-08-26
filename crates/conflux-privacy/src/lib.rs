//! Local DP (clip + noise) + epsilon accounting.
//!
//! See `docs/spec/conflux-spec-v1.md` §5–§6.

use conflux_config::{StrategyEntry, StrategyKind};
use rand_distr::{Distribution, Normal};

/// What varies about a privacy mechanism: how it transforms one client's
/// update before it leaves the client. `Box<dyn PrivacyMechanism>`
/// (Phase 11b) is why `transform`/`add_noise` take `&mut dyn rand::Rng`
/// rather than a generic `impl rand::Rng` — a trait object can't have a
/// generic method, and this is a zero-cost, automatic unsized coercion
/// at every existing call site (passing `&mut StdRng` where `&mut dyn
/// Rng` is expected needs no change from the caller).
pub trait PrivacyMechanism: Send + Sync {
    fn transform(&self, weights: &mut [f32], rng: &mut dyn rand::Rng);
}

// Phase 11b: registers this family's one member into `conflux-config`'s
// compile-time strategy registry (ADR 0002) — the third of the three
// spec §5 families now wired the same way `aggregator`/`selector` were
// in Phase 10b.
inventory::submit! {
    StrategyEntry { kind: StrategyKind::PrivacyMechanism, name: "gaussian_clipping" }
}

#[derive(Debug, thiserror::Error)]
pub enum PrivacyMechanismBuildError {
    #[error(
        "unknown privacy mechanism \"{0}\" — not a registered conflux-privacy strategy \
         (known: \"gaussian_clipping\")"
    )]
    Unknown(String),
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
/// Spec §5. References: Abadi et al. (2016), *Deep Learning with
/// Differential Privacy*, ACM CCS; Geyer, Klein & Nabi (2017),
/// *Differentially Private Federated Learning: A Client Level Perspective*.
#[derive(Debug, Clone, Copy)]
pub struct GaussianClippingPrivacy {
    pub clip_norm: f32,
    pub noise_multiplier: f32,
}

impl Default for GaussianClippingPrivacy {
    /// `clip_norm = 1.0`, `noise_multiplier = 1.0` — both widely used
    /// DP-SGD starting points (spec §5).
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
    /// `&mut dyn Rng` rather than a generic `impl Rng` (Phase 11b): an
    /// unsized trait-object parameter here is what lets
    /// `PrivacyMechanism::transform` (object-safe, backing `Box<dyn
    /// PrivacyMechanism>`) delegate straight to this method — a caller
    /// passing a concrete `&mut StdRng` still just works, via automatic
    /// unsized coercion.
    pub fn add_noise(&self, weights: &mut [f32], rng: &mut dyn rand::Rng) {
        let std_dev = (self.noise_multiplier * self.clip_norm) as f64;
        if std_dev == 0.0 {
            return;
        }
        let normal = Normal::new(0.0, std_dev).expect("std_dev > 0, checked above");
        for w in weights.iter_mut() {
            *w += normal.sample(rng) as f32;
        }
    }

    /// Clip then add noise — the full local-DP transform applied to one
    /// client's update before it leaves the client (spec §7/§8).
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
/// experiment's epsilon budget is exhausted. Spec §6.
pub trait PrivacyAccountant: Send + Sync {
    fn record_round(&mut self, noise_multiplier: f32, sample_rate: f32);
    fn current_epsilon(&self, delta: f64) -> f64;
    fn budget_exhausted(&self, target_epsilon: f64, delta: f64) -> bool;
}

/// The exact struct from spec §6. References: Mironov (2017), *Rényi
/// Differential Privacy*, IEEE CSF; Wang, Balle & Kasiviswanathan (2019),
/// *Subsampled Rényi Differential Privacy and Analytical Moments
/// Accountant*, AISTATS.
///
/// **Documented simplification** (see
/// `docs/phases/phase-2c-privacy.md`): per-round RDP is computed for the
/// *non-subsampled* Gaussian mechanism, ignoring `sample_rate`'s
/// privacy-amplification-by-subsampling effect. Subsampling only ever
/// *tightens* (lowers) the true epsilon for `sample_rate < 1`, so this
/// accountant reports a conservative upper bound, never an underestimate.
/// Exact subsampled RDP needs numerical-integration machinery out of scope
/// for Phase 2.
///
/// Phase 14 (`AccountingScope::PerClient`, ADR 0006): `client_rounds`
/// tracks each client's own round history *in addition to* the
/// experiment-wide `rounds` above — both are always recorded,
/// regardless of which scope is actually configured, so switching
/// `accounting_scope` between restarts never silently loses history one
/// scope was already accumulating. The RDP math itself
/// (`epsilon_from_rounds`) is shared between the two — `PerClient`
/// changes *which* history it's evaluated against, never the
/// composition itself (out of scope for this phase, see the phase
/// brief).
pub struct RdpAccountant {
    rounds: Vec<(f32, f32)>,
    client_rounds: std::collections::HashMap<String, Vec<(f32, f32)>>,
}

impl RdpAccountant {
    pub fn new() -> Self {
        Self {
            rounds: Vec::new(),
            client_rounds: std::collections::HashMap::new(),
        }
    }

    /// Records one round of exposure for a single client — called once
    /// per client actually admitted into a round's aggregate, when
    /// `accounting_scope = PerClient`. Independent of [`record_round`]
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
const RDP_ORDERS: &[f64] = &[
    1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0, 20.0, 24.0, 32.0, 48.0,
    64.0, 96.0, 128.0, 192.0, 256.0,
];

/// The RDP composition math itself (Mironov, 2017) — shared by
/// `current_epsilon` (the experiment-wide total) and
/// `current_epsilon_for_client` (Phase 14). Extracted once so
/// `PerClient` accounting is guaranteed to use *exactly* the same
/// composition as `Global`, just evaluated against a different history
/// — the one thing this phase's brief explicitly keeps out of scope is
/// touching this math at all.
fn epsilon_from_rounds(rounds: &[(f32, f32)], delta: f64) -> f64 {
    if rounds.is_empty() {
        return 0.0;
    }
    RDP_ORDERS
        .iter()
        .map(|&alpha| {
            let rdp_total: f64 = rounds
                .iter()
                .map(|&(noise_multiplier, _sample_rate)| {
                    // Non-subsampled Gaussian mechanism RDP at order α
                    // (Mironov, 2017): α / (2σ²).
                    alpha / (2.0 * (noise_multiplier as f64).powi(2))
                })
                .sum();
            // RDP → (ε, δ)-DP conversion (Mironov, 2017):
            // ε(α) = RDP(α) + ln(1/δ)/(α−1).
            rdp_total + (1.0 / delta).ln() / (alpha - 1.0)
        })
        .fold(f64::INFINITY, f64::min)
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
    fn every_buildable_name_is_also_registry_visible() {
        assert!(build_privacy_mechanism("gaussian_clipping", 1.0, 1.0).is_ok());
        assert!(
            conflux_config::lookup(StrategyKind::PrivacyMechanism, "gaussian_clipping").is_some()
        );
    }

    #[test]
    fn registry_constructed_mechanism_behaves_like_the_concrete_type() {
        use rand::SeedableRng;
        use rand::rngs::StdRng;

        let mechanism = build_privacy_mechanism("gaussian_clipping", 1.0, 0.0).unwrap();
        let mut weights = vec![3.0, 4.0]; // L2 norm 5.0
        let mut rng = StdRng::seed_from_u64(1);

        mechanism.transform(&mut weights, &mut rng);

        assert!((l2_norm(&weights) - 1.0).abs() < 1e-5);
    }
    use rand::SeedableRng;
    use rand::rngs::StdRng;

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

    // Phase 14: PerClient accounting.

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
