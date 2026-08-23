//! Shared math for crafting attacks: decoding a batch of honest updates
//! into per-coordinate statistics, and the inverse standard normal CDF
//! `AlieAttack` needs.

use conflux_proto::ClientDelta;

/// Decodes every update's weights — attacks operate on an "omniscient"
/// view of the honest batch (standard threat model in this literature:
/// the attacker sees what honest clients submitted before crafting its
/// own update), so this is simpler than `conflux-core`'s
/// `decode_and_validate` — it doesn't need to produce a Conflux error
/// type, just panic on genuinely malformed test input, which a crafted
/// attack's own inputs should never be.
pub(crate) fn decode_all(updates: &[ClientDelta]) -> Vec<Vec<f32>> {
    updates
        .iter()
        .map(|u| {
            conflux_proto::decode_weights(&u.weights)
                .expect("attack inputs must be well-formed encode_weights output")
        })
        .collect()
}

/// Per-coordinate mean across a batch of already-decoded weight vectors.
/// Panics on an empty batch — crafting an attack against zero honest
/// updates isn't a meaningful scenario.
pub(crate) fn coordinate_means(decoded: &[Vec<f32>]) -> Vec<f32> {
    let dim = decoded[0].len();
    let n = decoded.len() as f32;
    (0..dim)
        .map(|k| decoded.iter().map(|v| v[k]).sum::<f32>() / n)
        .collect()
}

/// Per-coordinate population standard deviation — Baruch, Baruch &
/// Goldberg (2019)'s ALIE attack is defined directly in terms of this
/// (population, not sample, std — the attacker is assumed to see the
/// entire honest population for the round, not a sample of it).
pub(crate) fn coordinate_std_devs(decoded: &[Vec<f32>], means: &[f32]) -> Vec<f32> {
    let dim = means.len();
    let n = decoded.len() as f32;
    (0..dim)
        .map(|k| {
            let variance = decoded
                .iter()
                .map(|v| {
                    let d = v[k] - means[k];
                    d * d
                })
                .sum::<f32>()
                / n;
            variance.sqrt()
        })
        .collect()
}

/// Inverse standard normal CDF (probit function) — Peter John Acklam's
/// rational approximation (2003), accuracy ~1.15e-9. Public-domain
/// algorithm, implemented directly rather than adding a statistics
/// dependency for this one function; `AlieAttack` is the only caller.
/// `p` must be in `(0, 1)`.
pub(crate) fn inverse_normal_cdf(p: f64) -> f64 {
    debug_assert!(
        p > 0.0 && p < 1.0,
        "inverse_normal_cdf: p out of range: {p}"
    );

    const A: [f64; 6] = [
        -3.969683028665376e+01,
        2.209460984245205e+02,
        -2.759285104469687e+02,
        1.383_577_518_672_69e2,
        -3.066479806614716e+01,
        2.506628277459239e+00,
    ];
    const B: [f64; 5] = [
        -5.447609879822406e+01,
        1.615858368580409e+02,
        -1.556989798598866e+02,
        6.680131188771972e+01,
        -1.328068155288572e+01,
    ];
    const C: [f64; 6] = [
        -7.784894002430293e-03,
        -3.223964580411365e-01,
        -2.400758277161838e+00,
        -2.549732539343734e+00,
        4.374664141464968e+00,
        2.938163982698783e+00,
    ];
    const D: [f64; 4] = [
        7.784695709041462e-03,
        3.224671290700398e-01,
        2.445134137142996e+00,
        3.754408661907416e+00,
    ];
    const P_LOW: f64 = 0.02425;
    let p_high = 1.0 - P_LOW;

    if p < P_LOW {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= p_high {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delta(weights: &[f32]) -> ClientDelta {
        ClientDelta {
            client_id: "test".to_string(),
            round: 1,
            weights: conflux_proto::encode_weights(weights),
            num_samples: 1,
        }
    }

    #[test]
    fn coordinate_means_matches_hand_computed_average() {
        let decoded = decode_all(&[delta(&[1.0, 10.0]), delta(&[3.0, 20.0])]);
        assert_eq!(coordinate_means(&decoded), vec![2.0, 15.0]);
    }

    #[test]
    fn coordinate_std_devs_is_zero_for_identical_values() {
        let decoded = decode_all(&[delta(&[5.0]), delta(&[5.0]), delta(&[5.0])]);
        let means = coordinate_means(&decoded);
        assert_eq!(coordinate_std_devs(&decoded, &means), vec![0.0]);
    }

    #[test]
    fn coordinate_std_devs_matches_hand_computed_population_std() {
        // [2, 4, 4, 4, 5, 5, 7, 9] has population std dev 2.0 (textbook example).
        let decoded = decode_all(&[
            delta(&[2.0]),
            delta(&[4.0]),
            delta(&[4.0]),
            delta(&[4.0]),
            delta(&[5.0]),
            delta(&[5.0]),
            delta(&[7.0]),
            delta(&[9.0]),
        ]);
        let means = coordinate_means(&decoded);
        let stds = coordinate_std_devs(&decoded, &means);
        assert!((stds[0] - 2.0).abs() < 1e-4, "got {stds:?}");
    }

    #[test]
    fn inverse_normal_cdf_matches_known_values() {
        assert!((inverse_normal_cdf(0.5) - 0.0).abs() < 1e-6);
        assert!((inverse_normal_cdf(0.975) - 1.959964).abs() < 1e-4);
        assert!((inverse_normal_cdf(0.995) - 2.575829).abs() < 1e-4);
        assert!((inverse_normal_cdf(0.8413447) - 1.0).abs() < 1e-4);
        // symmetry: Phi^-1(p) = -Phi^-1(1-p)
        assert!((inverse_normal_cdf(0.025) + inverse_normal_cdf(0.975)).abs() < 1e-6);
    }
}
