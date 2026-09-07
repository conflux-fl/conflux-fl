//! What a robust method's paper requires of the batch it aggregates.
//!
//! Several robust aggregators state a minimum batch size in terms of how
//! many Byzantine submissions they tolerate: Krum needs `n >= 2f + 3`,
//! Bulyan `n >= 4f + 3`, and the trimmed mean has to keep at least one
//! value after trimming `f` from each side. Below that, the method still
//! computes something — the implementations floor and clamp rather than
//! refusing — but the number is outside the regime its citation covers.
//!
//! This lives here, and not in `conflux-core` beside the methods
//! themselves, because `conflux-core` depends on `conflux-config` rather
//! than the other way round. Two callers need it: configuration
//! validation, which checks it against a configured `quorum` before
//! anything starts, and the round loop, which checks it against the
//! batch that actually arrived.
//!
//! One copy, because the same arithmetic reported two different ways is
//! how the two answers drift apart.

/// What a method's citation requires of a batch of size `n`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchRequirement {
    /// The smallest batch the citation's guarantee covers, at this `n`
    /// and Byzantine fraction.
    pub required: u64,
    /// How many submissions the implementation will exclude or trim at
    /// this batch size — the `f` the requirement is stated in terms of.
    pub excluded: u64,
    /// The paper's own statement of the rule, for the message.
    pub citation: &'static str,
}

impl BatchRequirement {
    /// Whether a batch of `n` satisfies it.
    pub fn satisfied_by(&self, n: u32) -> bool {
        u64::from(n) >= self.required
    }
}

/// How many submissions the implementation excludes at batch size `n`.
///
/// Mirrors `conflux-core`'s own `byzantine_count` exactly — floor, then
/// capped at `n - 1` so a method always has something left to aggregate.
/// Duplicated across the crate boundary rather than shared, because
/// `conflux-core` depends on this crate and the edge cannot be reversed;
/// the cap and the floor are pinned by a test on both sides.
fn excluded_at(byzantine_fraction: f32, n: u32) -> u64 {
    ((byzantine_fraction * n as f32).floor() as u64).min(u64::from(n).saturating_sub(1))
}

/// What `aggregator` requires of a batch of size `n`, or `None` when the
/// method states no batch minimum.
///
/// `None` is the common case and is not a gap: `fedavg` touches the whole
/// batch, `median` needs only that one value exists, and neither paper
/// states a size below which its claim lapses.
pub fn batch_requirement(
    aggregator: &str,
    byzantine_fraction: f32,
    n: u32,
) -> Option<BatchRequirement> {
    // A fraction outside [0, 1] is meaningless, and validation reports it
    // as an error in its own right — deriving a requirement from it here
    // would report the same problem twice, in worse words.
    if n == 0 || !(0.0..=1.0).contains(&byzantine_fraction) {
        return None;
    }
    let excluded = excluded_at(byzantine_fraction, n);
    let (required, citation): (u64, &str) = match aggregator {
        // Blanchard, El Mhamdi, Guerraoui & Stainer (2017).
        "krum" | "multi_krum" => (2 * excluded + 3, "Krum requires n ≥ 2f + 3"),
        // El Mhamdi, Guerraoui & Rouault (2018).
        "bulyan" => (4 * excluded + 3, "Bulyan requires n ≥ 4f + 3"),
        // Yin, Chen, Ramchandran & Bartlett (2018): trimming `f` from
        // each side must leave something behind.
        "trimmed_mean" => (2 * excluded + 1, "the trimmed mean must keep ≥ 1 value"),
        _ => return None,
    };
    Some(BatchRequirement {
        required,
        excluded,
        citation,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_with_no_stated_minimum_has_no_requirement() {
        // Not a gap in the table: neither paper states a batch size
        // below which its claim lapses.
        assert!(batch_requirement("fedavg", 0.3, 10).is_none());
        assert!(batch_requirement("median", 0.3, 10).is_none());
    }

    #[test]
    fn krum_at_the_documented_example() {
        // The case the configuration catalog prints: n = 4, fraction
        // 0.3 excludes f = 1, so the citation wants 5.
        let r = batch_requirement("krum", 0.3, 4).unwrap();
        assert_eq!((r.excluded, r.required), (1, 5));
        assert!(!r.satisfied_by(4));
        assert!(r.satisfied_by(5));
    }

    #[test]
    fn bulyan_needs_four_times_the_excluded_count_plus_three() {
        let r = batch_requirement("bulyan", 0.125, 8).unwrap();
        // floor(0.125 × 8) = 1, so 4·1 + 3 = 7, and 8 satisfies it —
        // the setting `baselines/bulyan-elmhamdi-2018` pins.
        assert_eq!((r.excluded, r.required), (1, 7));
        assert!(r.satisfied_by(8));
    }

    #[test]
    fn the_excluded_count_is_capped_below_the_batch() {
        // At fraction 1.0 the floor would be `n`, leaving nothing to
        // aggregate; `conflux-core` caps at n − 1 and so does this.
        assert_eq!(excluded_at(1.0, 5), 4);
        // And a fraction outside the range yields no requirement rather
        // than a nonsensical one.
        assert!(batch_requirement("krum", 1.5, 10).is_none());
    }

    #[test]
    fn an_empty_batch_has_no_requirement_to_check() {
        // Nothing arrived, which is an empty-batch condition the
        // aggregator reports itself; deriving a requirement from n = 0
        // would report it a second time.
        assert!(batch_requirement("krum", 0.3, 0).is_none());
    }
}
