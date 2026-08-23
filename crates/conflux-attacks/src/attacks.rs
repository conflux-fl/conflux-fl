//! Cited implementations of published FL attacks — see
//! `docs/phases/phase-12-attack-simulation.md` for the full source list
//! and scope notes. Each attack is "omniscient": `craft` sees the
//! honest batch before producing malicious updates, the strongest and
//! most conservative threat model this literature studies.

use conflux_proto::ClientDelta;

use crate::Attack;
use crate::stats::{coordinate_means, coordinate_std_devs, decode_all, inverse_normal_cdf};

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
}
