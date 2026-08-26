//! Cited implementations of published FL attacks — see
//! `docs/phases/phase-12-attack-simulation.md` for the full source list
//! and scope notes. Each attack is "omniscient": `craft` sees the
//! honest batch before producing malicious updates, the strongest and
//! most conservative threat model this literature studies.

use std::sync::Mutex;

use conflux_proto::ClientDelta;

use crate::stats::{coordinate_means, coordinate_std_devs, decode_all, inverse_normal_cdf};
use crate::{Attack, RoundFeedback};

fn l2_norm(v: &[f32]) -> f32 {
    v.iter().map(|x| x * x).sum::<f32>().sqrt()
}

fn l2_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

fn make_delta(client_id: String, round: u64, num_samples: u64, weights: &[f32]) -> ClientDelta {
    ClientDelta {
        client_id,
        round,
        weights: conflux_proto::encode_weights(weights),
        num_samples,
    }
}

/// Every attacker "blends in" using the same round and sample count the
/// honest batch reports — attackers here are challenging the
/// *aggregation* rule, not exploiting `conflux-buffer`'s round-matching
/// or `FedAvg`'s sample-count weighting, which are separate concerns.
fn round_and_samples(honest_updates: &[ClientDelta]) -> (u64, u64) {
    let first = honest_updates
        .first()
        .expect("craft is only meaningful against a non-empty honest batch");
    (first.round, first.num_samples)
}

/// Submits i.i.d. Gaussian noise instead of a real update — the generic
/// "arbitrary/omniscient Byzantine failure" threat model every
/// robustness paper tests against first.
///
/// Blanchard, El Mhamdi, Guerraoui & Stainer (2017), *Machine Learning
/// with Adversaries: Byzantine Tolerant Gradient Descent*, NeurIPS 2017.
pub struct GaussianAttack {
    pub std_dev: f32,
    /// Seeded for reproducible tests — an OS-seeded variant isn't needed
    /// here, unlike `conflux-privacy`'s noise (which must vary run to
    /// run in production); this crate is test/dev-only (ADR 0010).
    pub seed: u64,
}

impl Attack for GaussianAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        if num_attackers == 0 {
            return Vec::new();
        }
        use rand::SeedableRng;
        use rand::rngs::StdRng;
        use rand_distr::{Distribution, Normal};

        let decoded = decode_all(honest_updates);
        let dim = decoded[0].len();
        let (round, num_samples) = round_and_samples(honest_updates);
        let mut rng = StdRng::seed_from_u64(self.seed);
        let normal = Normal::new(0.0, self.std_dev as f64).expect("std_dev must be > 0");

        (0..num_attackers)
            .map(|i| {
                let weights: Vec<f32> = (0..dim).map(|_| normal.sample(&mut rng) as f32).collect();
                make_delta(
                    format!("gaussian-attacker-{i}"),
                    round,
                    num_samples,
                    &weights,
                )
            })
            .collect()
    }
}

/// Negates and scales the honest consensus direction — a coordinated
/// push *away* from the true update, not just noise.
///
/// Li, Xu, Chen & Charles (2019), *RSA: Byzantine-Robust Stochastic
/// Aggregation Methods for Distributed Learning from Heterogeneous
/// Datasets*, AAAI 2019.
pub struct SignFlippingAttack {
    pub scale: f32,
}

impl Attack for SignFlippingAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        if num_attackers == 0 {
            return Vec::new();
        }
        let decoded = decode_all(honest_updates);
        let means = coordinate_means(&decoded);
        let (round, num_samples) = round_and_samples(honest_updates);
        let malicious: Vec<f32> = means.iter().map(|m| -self.scale * m).collect();

        (0..num_attackers)
            .map(|i| {
                make_delta(
                    format!("signflip-attacker-{i}"),
                    round,
                    num_samples,
                    &malicious,
                )
            })
            .collect()
    }
}

/// "A Little Is Enough": shifts each coordinate by a calibrated multiple
/// of the honest updates' own population standard deviation — small
/// enough to look like a plausible honest update rather than an
/// obvious outlier, specifically designed to evade distance-based
/// (Krum/Multi-Krum) and coordinate-trimming (Trimmed Mean/Median)
/// defenses. The shift `z` is derived from Algorithm 1 of the paper via
/// the inverse standard normal CDF (`stats::inverse_normal_cdf`).
///
/// Baruch, Baruch & Goldberg (2019), *A Little Is Enough: Circumventing
/// Defenses For Distributed Learning*, NeurIPS 2019.
pub struct AlieAttack;

impl AlieAttack {
    /// Algorithm 1's `z_max`: the largest per-coordinate shift (in
    /// honest standard deviations) that a majority-style argument still
    /// can't distinguish from the honest population, given `num_total`
    /// participants and `num_attackers` colluding ones.
    fn z(num_total: usize, num_attackers: usize) -> f64 {
        let n = num_total as f64;
        let m = num_attackers as f64;
        let denom = n - m;
        if denom <= 0.0 {
            // Attackers are the whole population or more — degenerate,
            // not a scenario this formula is defined for.
            return 0.0;
        }
        let s = (n / 2.0 + 1.0).floor() - m;
        let p = ((n - m - s) / denom).clamp(1e-9, 1.0 - 1e-9);
        inverse_normal_cdf(p)
    }
}

impl Attack for AlieAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        if num_attackers == 0 {
            return Vec::new();
        }
        let decoded = decode_all(honest_updates);
        let means = coordinate_means(&decoded);
        let stds = coordinate_std_devs(&decoded, &means);
        let (round, num_samples) = round_and_samples(honest_updates);

        let num_total = honest_updates.len() + num_attackers;
        let z = Self::z(num_total, num_attackers) as f32;
        let malicious: Vec<f32> = means
            .iter()
            .zip(&stds)
            .map(|(mean, std)| mean - z * std)
            .collect();

        (0..num_attackers)
            .map(|i| make_delta(format!("alie-attacker-{i}"), round, num_samples, &malicious))
            .collect()
    }
}

/// Boosts a chosen malicious direction so it dominates `FedAvg`'s
/// average despite being one update among many — the mechanism behind
/// model-replacement/backdoor attacks. **Documented scope-narrowing**:
/// the source paper replaces the *entire model* across a carefully
/// timed sequence of rounds; this adapts the same boosting mechanism to
/// one round's delta aggregation, which is what `conflux-core::Aggregator`
/// actually operates on — not a claim of full paper reproduction.
///
/// Bagdasaryan, Veit, Hua, Estrin & Shmatikov (2020), *How To Backdoor
/// Federated Learning*, AISTATS 2020.
pub struct ScalingAttack {
    pub scale_factor: f32,
    /// The coordinates the attacker wants the aggregate pulled toward.
    /// Must be the same length as the honest updates' weight vectors.
    pub malicious_direction: Vec<f32>,
}

impl Attack for ScalingAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        if num_attackers == 0 {
            return Vec::new();
        }
        let decoded = decode_all(honest_updates);
        let means = coordinate_means(&decoded);
        assert_eq!(
            means.len(),
            self.malicious_direction.len(),
            "ScalingAttack::malicious_direction must match the honest updates' dimension"
        );
        let (round, num_samples) = round_and_samples(honest_updates);

        let boosted: Vec<f32> = means
            .iter()
            .zip(&self.malicious_direction)
            .map(|(mean, target)| mean + self.scale_factor * (target - mean))
            .collect();

        (0..num_attackers)
            .map(|i| {
                make_delta(
                    format!("scaling-attacker-{i}"),
                    round,
                    num_samples,
                    &boosted,
                )
            })
            .collect()
    }
}

/// A colluding Sybil cluster that submits the **same fixed update every
/// round**, regardless of how the honest batch evolves — unlike every
/// other attack in this module, whose crafted output is a function of
/// *that round's* honest batch (`GaussianAttack`'s noise aside, which is
/// randomized rather than batch-dependent, but still not *consistent*
/// round to round). `ScalingAttack`, for instance, chases the honest
/// mean as it shifts (`mean + scale_factor * (target - mean)`) — its
/// raw output changes round to round even with fixed parameters, as the
/// model converges.
///
/// This attack exists specifically to stress-test **temporal** defenses
/// (`conflux-core::FoolsGoldAggregator`, and the Deviation Stability
/// Scoring hypothesis in
/// `docs/research/temporal-consistency-aggregation.md`) — a stable,
/// self-similar, round-over-round signature from a colluding cluster is
/// exactly the pattern those defenses are built to catch, and exactly
/// what every single-round-only attack above can't model, since none of
/// them are consistent with their own past selves across rounds.
///
/// Not itself a cited attack from a specific paper — a deliberately
/// simple, worst-case-for-the-defense construction (identical Sybils are
/// the easiest possible case for a similarity-based temporal defense to
/// catch; if a defense can't catch *this*, subtler collusion is out of
/// reach too), same spirit as `GaussianAttack`'s role as the generic
/// baseline every robustness paper tests against first.
pub struct PersistentSybilAttack {
    /// The exact update every colluding attacker submits, every round.
    pub fixed_update: Vec<f32>,
}

impl Attack for PersistentSybilAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        if num_attackers == 0 {
            return Vec::new();
        }
        let (round, num_samples) = round_and_samples(honest_updates);

        (0..num_attackers)
            .map(|i| {
                make_delta(
                    format!("sybil-attacker-{i}"),
                    round,
                    num_samples,
                    &self.fixed_update,
                )
            })
            .collect()
    }
}

/// A reactive attacker that observes how the *previous* round actually
/// went and adapts its magnitude accordingly — escalating when its
/// updates seem to be getting through largely unresisted, retreating
/// when they seem to be getting actively suppressed *beyond ordinary
/// dilution*. Every other attack in this module is a fixed, non-adaptive
/// strategy — the standard, conservative "omniscient but non-reactive"
/// threat model most robustness papers evaluate against first. This one
/// specifically models a *reactive* adversary, closing part of the gap
/// `docs/research/temporal-consistency-aggregation.md`'s Section 2.2
/// flagged as a stretch goal: whether a temporal defense still holds
/// against an attacker that's also adapting round to round, not just a
/// defense reacting to a static attack.
///
/// **v2, revised from a v1 bug the research proposal documented rather
/// than hid** (§5.3, Finding 3): comparing the previous round's
/// `‖aggregate − submission‖ / ‖submission‖` against a fixed threshold
/// alone can't distinguish "a real defense pulled me back" from "I'm a
/// minority of the batch, so *any* weighted average dilutes me" — v1
/// retreated even against completely undefended `FedAvg`, since sheer
/// minority-share dilution already exceeded its threshold. v2 instead
/// computes the **expected** dilution a plain, undefended weighted
/// average would have produced from last round's honest batch and
/// submission, and only treats the actual outcome as "suppressed" if it
/// deviates from *that* baseline by more than `suppression_margin` —
/// isolating an active defense's effect from ordinary averaging
/// arithmetic.
///
/// **Not a reproduction of a specific published optimization-based
/// attack** (e.g. Fang, Cao, Jia, Gong & Liu, 2020's gradient-based
/// search over a defense's own decision boundary, *Local Model
/// Poisoning Attacks to Byzantine-Robust Federated Learning*, USENIX
/// Security 2020) — still a simpler, deliberately transparent local
/// hill-climbing heuristic, easy to verify deterministically. A real
/// optimization-based attack search is flagged as separate, harder
/// future work in that document, not claimed here.
pub struct AdaptiveEvasionAttack {
    /// Attacked direction — not required to be a unit vector, scaled by
    /// the adapting magnitude below.
    pub direction: Vec<f32>,
    pub escalation_factor: f32,
    pub retreat_factor: f32,
    /// How much *worse* than plain-averaging dilution the actual
    /// suppression must be (as an additional fraction of
    /// `‖submission‖`) to count as "a real defense is active" rather
    /// than ordinary minority-share dilution.
    pub suppression_margin: f32,
    current_magnitude: Mutex<f32>,
    /// What the honest batch's (sample-count-weighted) mean and total
    /// weight were last round — remembered so this round can compute
    /// what an undefended average *would* have produced, without
    /// needing `RoundFeedback` itself extended (every other attack, and
    /// any future one, still only needs `previous_submission`/
    /// `previous_aggregate`).
    last_honest_mean_and_weight: Mutex<Option<(Vec<f32>, f32)>>,
}

impl AdaptiveEvasionAttack {
    /// Reasonable defaults for `escalation_factor`/`retreat_factor`/
    /// `suppression_margin` — tunable via the public fields afterward if
    /// a specific experiment needs a different climb/retreat rate.
    pub fn new(direction: Vec<f32>, initial_magnitude: f32) -> Self {
        Self {
            direction,
            escalation_factor: 1.2,
            retreat_factor: 0.5,
            suppression_margin: 0.15,
            current_magnitude: Mutex::new(initial_magnitude),
            last_honest_mean_and_weight: Mutex::new(None),
        }
    }
}

/// Sample-count-weighted mean and total weight of a batch — the
/// undefended-`FedAvg`-equivalent this attack needs to compute what
/// "just dilution, no active defense" would have produced.
fn weighted_mean_and_weight(updates: &[ClientDelta]) -> (Vec<f32>, f32) {
    let decoded = decode_all(updates);
    let dim = decoded.first().map_or(0, |d| d.len());
    let mut mean = vec![0.0f32; dim];
    let mut total_weight = 0.0f32;
    for (u, w) in updates.iter().zip(&decoded) {
        let weight = u.num_samples as f32;
        total_weight += weight;
        for (m, x) in mean.iter_mut().zip(w) {
            *m += weight * x;
        }
    }
    if total_weight > 0.0 {
        for m in &mut mean {
            *m /= total_weight;
        }
    }
    (mean, total_weight)
}

impl Attack for AdaptiveEvasionAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        self.craft_adaptive(honest_updates, num_attackers, None)
    }

    fn craft_adaptive(
        &self,
        honest_updates: &[ClientDelta],
        num_attackers: usize,
        feedback: Option<&RoundFeedback>,
    ) -> Vec<ClientDelta> {
        if num_attackers == 0 {
            return Vec::new();
        }

        let magnitude = {
            let mut current = self
                .current_magnitude
                .lock()
                .expect("AdaptiveEvasionAttack magnitude mutex poisoned");
            let mut last_honest = self
                .last_honest_mean_and_weight
                .lock()
                .expect("AdaptiveEvasionAttack honest-mean mutex poisoned");

            if let (Some(fb), Some((last_mean, last_honest_weight))) =
                (feedback, last_honest.as_ref())
            {
                let submission_norm = l2_norm(&fb.previous_submission).max(1e-6);
                let actual_pulled_fraction =
                    l2_distance(&fb.previous_aggregate, &fb.previous_submission) / submission_norm;

                // What an undefended, sample-count-weighted average
                // (num_attackers attackers, each weighted the same as
                // one honest client's total weight share) would have
                // produced from last round's honest batch and this
                // attacker's submission — pure dilution, no filtering.
                let attacker_weight = *last_honest_weight * (num_attackers as f32)
                    / (honest_updates.len().max(1) as f32);
                let total = last_honest_weight + attacker_weight;
                let expected: Vec<f32> = last_mean
                    .iter()
                    .zip(&fb.previous_submission)
                    .map(|(h, s)| (last_honest_weight * h + attacker_weight * s) / total)
                    .collect();
                let expected_pulled_fraction =
                    l2_distance(&expected, &fb.previous_submission) / submission_norm;

                if actual_pulled_fraction <= expected_pulled_fraction + self.suppression_margin {
                    // No worse than plain dilution alone would explain —
                    // nothing actively suppressing beyond that. Push harder.
                    *current *= self.escalation_factor;
                } else {
                    // Pulled back further than dilution alone accounts
                    // for — a real defense is doing something. Retreat.
                    *current *= self.retreat_factor;
                }
            }

            *last_honest = Some(weighted_mean_and_weight(honest_updates));
            *current
        };

        let crafted: Vec<f32> = self.direction.iter().map(|d| d * magnitude).collect();
        let (round, num_samples) = round_and_samples(honest_updates);

        (0..num_attackers)
            .map(|i| {
                make_delta(
                    format!("adaptive-attacker-{i}"),
                    round,
                    num_samples,
                    &crafted,
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn honest_delta(client_id: &str, weights: &[f32]) -> ClientDelta {
        ClientDelta {
            client_id: client_id.to_string(),
            round: 3,
            weights: conflux_proto::encode_weights(weights),
            num_samples: 10,
        }
    }

    #[test]
    fn gaussian_attack_produces_the_right_count_and_shape() {
        let honest = vec![
            honest_delta("a", &[1.0, 2.0]),
            honest_delta("b", &[1.1, 2.1]),
        ];
        let attack = GaussianAttack {
            std_dev: 1.0,
            seed: 42,
        };

        let crafted = attack.craft(&honest, 3);

        assert_eq!(crafted.len(), 3);
        for delta in &crafted {
            assert_eq!(delta.round, 3);
            let w = conflux_proto::decode_weights(&delta.weights).unwrap();
            assert_eq!(w.len(), 2);
        }
    }

    #[test]
    fn gaussian_attack_with_zero_attackers_returns_nothing() {
        let honest = vec![honest_delta("a", &[1.0])];
        let attack = GaussianAttack {
            std_dev: 1.0,
            seed: 1,
        };
        assert!(attack.craft(&honest, 0).is_empty());
    }

    #[test]
    fn gaussian_attack_is_reproducible_for_the_same_seed() {
        let honest = vec![honest_delta("a", &[1.0, 2.0, 3.0])];
        let attack = GaussianAttack {
            std_dev: 5.0,
            seed: 7,
        };

        let first = attack.craft(&honest, 1);
        let second = attack.craft(&honest, 1);

        assert_eq!(first[0].weights, second[0].weights);
    }

    #[test]
    fn sign_flipping_attack_negates_the_honest_mean() {
        let honest = vec![
            honest_delta("a", &[2.0, -4.0]),
            honest_delta("b", &[2.0, -4.0]),
        ];
        let attack = SignFlippingAttack { scale: 1.0 };

        let crafted = attack.craft(&honest, 1);

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert_eq!(w, vec![-2.0, 4.0]);
    }

    #[test]
    fn sign_flipping_attack_scale_multiplies_the_flip() {
        let honest = vec![honest_delta("a", &[1.0])];
        let attack = SignFlippingAttack { scale: 3.0 };

        let crafted = attack.craft(&honest, 1);

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert_eq!(w, vec![-3.0]);
    }

    #[test]
    fn alie_attack_shifts_below_the_honest_mean_by_z_std_devs() {
        // Values chosen so mean=5, population std dev=2 exactly (see
        // stats.rs's own test for the same textbook example).
        let honest = vec![
            honest_delta("a", &[2.0]),
            honest_delta("b", &[4.0]),
            honest_delta("c", &[4.0]),
            honest_delta("d", &[4.0]),
            honest_delta("e", &[5.0]),
            honest_delta("f", &[5.0]),
            honest_delta("g", &[7.0]),
            honest_delta("h", &[9.0]),
        ];
        let attack = AlieAttack;

        let crafted = attack.craft(&honest, 1);

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        let z = AlieAttack::z(9, 1); // 8 honest + 1 attacker
        let expected = 5.0 - (z as f32) * 2.0;
        assert!(
            (w[0] - expected).abs() < 1e-3,
            "got {w:?}, expected {expected}"
        );
    }

    #[test]
    fn alie_z_grows_as_the_attacker_fraction_grows() {
        // More attackers (relative to n) can push further while still
        // being statistically plausible — z should increase.
        let z_few = AlieAttack::z(100, 5);
        let z_many = AlieAttack::z(100, 30);
        assert!(z_many > z_few, "z_few={z_few}, z_many={z_many}");
    }

    #[test]
    fn scaling_attack_scale_zero_reproduces_the_honest_mean() {
        let honest = vec![
            honest_delta("a", &[1.0, 2.0]),
            honest_delta("b", &[3.0, 4.0]),
        ];
        let attack = ScalingAttack {
            scale_factor: 0.0,
            malicious_direction: vec![100.0, 100.0],
        };

        let crafted = attack.craft(&honest, 1);

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert_eq!(w, vec![2.0, 3.0]); // the honest mean, untouched
    }

    #[test]
    fn scaling_attack_scale_one_reaches_the_malicious_direction_exactly() {
        let honest = vec![honest_delta("a", &[1.0, 2.0])];
        let attack = ScalingAttack {
            scale_factor: 1.0,
            malicious_direction: vec![50.0, -50.0],
        };

        let crafted = attack.craft(&honest, 1);

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert_eq!(w, vec![50.0, -50.0]);
    }

    #[test]
    #[should_panic(expected = "must match the honest updates' dimension")]
    fn scaling_attack_dimension_mismatch_panics_clearly() {
        let honest = vec![honest_delta("a", &[1.0, 2.0])];
        let attack = ScalingAttack {
            scale_factor: 1.0,
            malicious_direction: vec![1.0], // wrong length
        };
        attack.craft(&honest, 1);
    }

    #[test]
    fn persistent_sybil_attack_produces_the_right_count_and_shape() {
        let honest = vec![honest_delta("a", &[1.0, 2.0])];
        let attack = PersistentSybilAttack {
            fixed_update: vec![9.0, 9.0],
        };

        let crafted = attack.craft(&honest, 3);

        assert_eq!(crafted.len(), 3);
        for delta in &crafted {
            let w = conflux_proto::decode_weights(&delta.weights).unwrap();
            assert_eq!(w, vec![9.0, 9.0]);
        }
    }

    #[test]
    fn persistent_sybil_attack_output_is_identical_regardless_of_the_honest_batch() {
        // The defining property this attack exists to test: unlike
        // ScalingAttack (which chases the honest mean), the crafted
        // output must not depend on what the honest batch looks like —
        // proof it stays self-consistent as the honest batch (and the
        // model it reflects) evolves round to round.
        let attack = PersistentSybilAttack {
            fixed_update: vec![7.0, -3.0],
        };
        let round_one = vec![honest_delta("a", &[1.0, 2.0])];
        let round_two = vec![
            honest_delta("a", &[500.0, -500.0]),
            honest_delta("b", &[-500.0, 500.0]),
        ];

        let crafted_one = attack.craft(&round_one, 1);
        let crafted_two = attack.craft(&round_two, 1);

        let w1 = conflux_proto::decode_weights(&crafted_one[0].weights).unwrap();
        let w2 = conflux_proto::decode_weights(&crafted_two[0].weights).unwrap();
        assert_eq!(w1, w2);
        assert_eq!(w1, vec![7.0, -3.0]);
    }

    #[test]
    fn persistent_sybil_attack_zero_attackers_produces_nothing() {
        let honest = vec![honest_delta("a", &[1.0, 2.0])];
        let attack = PersistentSybilAttack {
            fixed_update: vec![9.0, 9.0],
        };

        let crafted = attack.craft(&honest, 0);

        assert!(crafted.is_empty());
    }

    // --- Adaptive evasion ---

    #[test]
    fn adaptive_evasion_first_round_uses_initial_magnitude_with_no_feedback() {
        let honest = vec![honest_delta("a", &[1.0, 2.0])];
        let attack = AdaptiveEvasionAttack::new(vec![1.0, 0.0], 10.0);

        let crafted = attack.craft_adaptive(&honest, 1, None);

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert_eq!(w, vec![10.0, 0.0]);
    }

    #[test]
    fn adaptive_evasion_escalates_when_pulled_no_worse_than_dilution_alone() {
        let honest = vec![honest_delta("a", &[1.0, 0.0])]; // weight 10
        let attack = AdaptiveEvasionAttack::new(vec![1.0, 0.0], 10.0);
        // Priming call: no feedback yet, but records this round's honest
        // mean/weight ([1,0], weight 10) for the next call to compare
        // against.
        attack.craft_adaptive(&honest, 1, None);

        // 1 attacker (weight 10, matching the single honest client) vs.
        // submission [10,0]: pure dilution alone would produce
        // (10*[1,0] + 10*[10,0]) / 20 = [5.5, 0], i.e. an "expected"
        // pulled fraction of 4.5/10 = 0.45. The aggregate landing at
        // [9.9,0] (actual pulled fraction 0.01) is far *better* than
        // that baseline for the attacker -> nothing beyond ordinary
        // dilution is suppressing it -> escalate by 1.2x.
        let feedback = RoundFeedback {
            previous_submission: vec![10.0, 0.0],
            previous_aggregate: vec![9.9, 0.0],
        };

        let crafted = attack.craft_adaptive(&honest, 1, Some(&feedback));

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert!(
            (w[0] - 12.0).abs() < 1e-3,
            "got {w:?}, expected magnitude 12.0 (10.0 * 1.2)"
        );
    }

    #[test]
    fn adaptive_evasion_retreats_when_pulled_worse_than_dilution_alone() {
        let honest = vec![honest_delta("a", &[1.0, 0.0])]; // weight 10
        let attack = AdaptiveEvasionAttack::new(vec![1.0, 0.0], 10.0);
        attack.craft_adaptive(&honest, 1, None); // primes last-honest state

        // Same setup as the escalate test above (expected pulled
        // fraction from dilution alone: 0.45), but the aggregate landed
        // at [1,0] — almost exactly the honest consensus, an actual
        // pulled fraction of 0.9, far worse than dilution alone would
        // explain -> a real defense is suppressing this -> retreat.
        let feedback = RoundFeedback {
            previous_submission: vec![10.0, 0.0],
            previous_aggregate: vec![1.0, 0.0],
        };

        let crafted = attack.craft_adaptive(&honest, 1, Some(&feedback));

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert!(
            (w[0] - 5.0).abs() < 1e-3,
            "got {w:?}, expected magnitude 5.0 (10.0 * 0.5)"
        );
    }

    #[test]
    fn adaptive_evasion_no_prior_round_never_retreats_or_escalates() {
        // Without a primed last-honest-mean (the very first round any
        // feedback could arrive for), there's nothing to compare
        // against yet -> magnitude stays untouched even if `feedback`
        // is `Some`, matching `craft_adaptive`'s guard requiring both.
        let honest = vec![honest_delta("a", &[1.0, 0.0])];
        let attack = AdaptiveEvasionAttack::new(vec![1.0, 0.0], 10.0);
        let feedback = RoundFeedback {
            previous_submission: vec![10.0, 0.0],
            previous_aggregate: vec![1.0, 0.0],
        };

        let crafted = attack.craft_adaptive(&honest, 1, Some(&feedback));

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert_eq!(w, vec![10.0, 0.0]);
    }

    #[test]
    fn adaptive_evasion_state_persists_and_compounds_across_calls() {
        let honest = vec![honest_delta("a", &[1.0, 0.0])];
        let attack = AdaptiveEvasionAttack::new(vec![1.0, 0.0], 10.0);
        let success_feedback = |submitted: f32| RoundFeedback {
            previous_submission: vec![submitted, 0.0],
            previous_aggregate: vec![submitted * 0.99, 0.0],
        };

        attack.craft_adaptive(&honest, 1, None); // round 0: magnitude 10.0
        attack.craft_adaptive(&honest, 1, Some(&success_feedback(10.0))); // -> 12.0
        let crafted = attack.craft_adaptive(&honest, 1, Some(&success_feedback(12.0))); // -> 14.4

        let w = conflux_proto::decode_weights(&crafted[0].weights).unwrap();
        assert!(
            (w[0] - 14.4).abs() < 1e-2,
            "got {w:?}, expected compounded magnitude 14.4 (10.0 * 1.2 * 1.2)"
        );
    }

    #[test]
    fn adaptive_evasion_zero_attackers_produces_nothing() {
        let honest = vec![honest_delta("a", &[1.0, 2.0])];
        let attack = AdaptiveEvasionAttack::new(vec![1.0, 0.0], 10.0);

        let crafted = attack.craft_adaptive(&honest, 0, None);

        assert!(crafted.is_empty());
    }
}
