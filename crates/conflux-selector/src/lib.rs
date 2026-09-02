//! Client sampling strategies: choosing which registered clients train
//! in a given round.
//!
//! Training every registered client every round doesn't scale — at
//! cross-device or crowdsource scale that's thousands of clients'
//! bandwidth and compute spent for no accuracy benefit over training a
//! random subset. This crate's whole job is picking that subset: given a
//! pool of candidate client IDs, return up to `n` of them for round
//! `round`.
//!
//! # Example
//!
//! ```
//! use conflux_selector::{SelectionSeed, build_selector};
//!
//! let pool: Vec<String> = (0..100).map(|i| format!("client-{i}")).collect();
//! let selector = build_selector("uniform_random", SelectionSeed::Fixed(42)).unwrap();
//!
//! let chosen = selector.select(&pool, 10, 1);
//! assert_eq!(chosen.len(), 10);
//!
//! // A fixed seed is reproducible for a given round — which is what
//! // makes a research run repeatable.
//! let again = build_selector("uniform_random", SelectionSeed::Fixed(42))
//!     .unwrap()
//!     .select(&pool, 10, 1);
//! assert_eq!(chosen, again);
//!
//! // ...but the round number is mixed in, so consecutive rounds do not
//! // train the same subset forever.
//! assert_ne!(chosen, selector.select(&pool, 10, 2));
//!
//! // Asking for more than exist yields everyone, not an error.
//! assert_eq!(selector.select(&pool, 500, 1).len(), pool.len());
//! ```

#![warn(missing_docs)]

use conflux_config::{StrategyEntry, StrategyKind};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;

/// How a selector seeds its RNG for one `select` call.
///
/// This mirrors `conflux-config`'s resolved `seed_mode`/`seed_value`
/// pair conceptually, but collapses the two into a single enum: `Fixed`
/// always carries its seed, so there's no way to end up holding the
/// illegal combination `seed_mode: Fixed, seed_value: None` that two
/// independent `Option` fields would otherwise allow. The caller
/// (typically `conflux-server`, right after resolving config) does the
/// small translation from the resolved config into this enum.
#[derive(Debug, Clone, Copy)]
pub enum SelectionSeed {
    /// Deterministic, combined with the round number so consecutive
    /// rounds don't select an identical subset.
    Fixed(u64),
    /// Seeded from OS entropy on every call; not reproducible run to
    /// run. The right choice whenever a predictable selection pattern
    /// would itself be a liability — e.g. a public/anonymous
    /// crowdsourced deployment, where a fixed seed would let anyone who
    /// knows it anticipate which clients get selected next.
    OsRandom,
}

/// Selects up to `n` clients from `candidates` for one round.
///
/// `&self` (not `&mut self`), plus an explicit `round` parameter, rather
/// than a `Mutex`-guarded RNG field: every implementation stays trivially
/// `Send + Sync`, and a fixed-seed selection is reproducible independent
/// of call order — re-running round 5 always gives the same subset.
pub trait ClientSelector: Send + Sync {
    /// Returns up to `n` of `candidates`. Fewer when the pool is smaller
    /// than `n`; never more, and never a client that wasn't a candidate.
    ///
    /// `round` is what makes a fixed-seed selection reproducible: an
    /// implementation derives its randomness from the seed *and* this
    /// number, so round 5 always picks the same subset no matter how many
    /// rounds preceded it in this process.
    fn select(&self, candidates: &[String], n: usize, round: u64) -> Vec<String>;
}

/// The one shipped selector, cited to McMahan, Moore, Ramage, Hampson &
/// y Arcas (2017), *Communication-Efficient Learning of Deep Networks
/// from Decentralized Data*, AISTATS — the client-sampling strategy from
/// the original FedAvg algorithm: pick `n` of the candidates, uniformly
/// at random.
pub struct UniformRandomSelector {
    /// Where the sampling randomness comes from — a fixed seed for a
    /// reproducible research run, or OS entropy for a deployment where a
    /// predictable selection would be exploitable.
    pub seed: SelectionSeed,
}

// Registers this family's one member into `conflux-config`'s
// compile-time strategy registry — see `conflux-core`'s analogous
// `inventory::submit!` for `Aggregator`s, or the registry's own doc
// comment in `conflux-config::registry`, for how this lets any crate
// linked into the final binary (this crate included, via
// `build_selector` below) check a configured name like
// `selector = "uniform_random"` against every submitted entry, without
// `conflux-config` ever importing this crate.
inventory::submit! {
    StrategyEntry {
        kind: StrategyKind::Selector,
        name: "uniform_random",
        citation: "McMahan, Moore, Ramage, Hampson & y Arcas (2017), Communication-Efficient Learning of Deep Networks from Decentralized Data",
        family: "selector",
        params: &[],
    }
}

#[derive(Debug, thiserror::Error)]
/// Why a selector name couldn't be turned into a `ClientSelector`.
pub enum SelectorBuildError {
    #[error(
        "unknown selector \"{0}\" — not a registered conflux-selector strategy \
         (known: \"uniform_random\")"
    )]
    /// The name isn't in this crate's registry. Almost always a typo in a
    /// resolved `selector` config value, since the set of valid names is
    /// fixed at compile time.
    Unknown(String),
}

/// Constructs the `ClientSelector` named by a resolved
/// `config.selector.value`. The one match arm today mirrors the one
/// `inventory::submit!` above.
pub fn build_selector(
    name: &str,
    seed: SelectionSeed,
) -> Result<Box<dyn ClientSelector>, SelectorBuildError> {
    match name {
        "uniform_random" => Ok(Box::new(UniformRandomSelector { seed })),
        other => Err(SelectorBuildError::Unknown(other.to_string())),
    }
}

impl ClientSelector for UniformRandomSelector {
    fn select(&self, candidates: &[String], n: usize, round: u64) -> Vec<String> {
        match self.seed {
            // `wrapping_add`, not a cryptographic hash — this only needs
            // to vary the selection across rounds, not resist an
            // adversary (that's what `OsRandom` in production is for).
            SelectionSeed::Fixed(seed) => {
                let mut rng = StdRng::seed_from_u64(seed.wrapping_add(round));
                candidates.sample(&mut rng, n).cloned().collect()
            }
            // `rand::rng()` is the OS-entropy-seeded thread-local
            // generator (rand 0.10's replacement for `thread_rng()`).
            SelectionSeed::OsRandom => {
                let mut rng = rand::rng();
                candidates.sample(&mut rng, n).cloned().collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("client-{i}")).collect()
    }

    #[test]
    fn same_seed_and_round_is_reproducible() {
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(42),
        };
        let pool = candidates(20);

        let first = selector.select(&pool, 5, 3);
        let second = selector.select(&pool, 5, 3);

        assert_eq!(first, second);
    }

    #[test]
    fn different_rounds_vary_the_selection() {
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(42),
        };
        let pool = candidates(20);

        let selections: std::collections::HashSet<_> = (0..5)
            .map(|round| selector.select(&pool, 5, round))
            .collect();

        assert!(selections.len() > 1, "expected selection to vary by round");
    }

    #[test]
    fn n_greater_than_pool_returns_every_candidate() {
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(1),
        };
        let pool = candidates(3);

        let selected = selector.select(&pool, 10, 0);

        assert_eq!(selected.len(), 3);
        for c in &pool {
            assert!(selected.contains(c));
        }
    }

    #[test]
    fn n_zero_returns_empty() {
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(1),
        };
        let pool = candidates(5);

        assert!(selector.select(&pool, 0, 0).is_empty());
    }

    #[test]
    fn selection_has_no_duplicates() {
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(7),
        };
        let pool = candidates(50);

        let selected = selector.select(&pool, 10, 0);
        let unique: std::collections::HashSet<_> = selected.iter().collect();

        assert_eq!(selected.len(), unique.len());
    }

    #[test]
    fn empty_candidate_pool_returns_empty_selection() {
        // A registry that's just evicted every client (or hasn't
        // registered any yet) can hand this crate an empty pool — this
        // isn't malicious input, but it's a real zero case that has to
        // resolve to "nobody trains this round," not a panic.
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(1),
        };

        assert!(selector.select(&[], 5, 0).is_empty());
    }

    #[test]
    fn duplicate_ids_in_the_pool_are_not_deduplicated() {
        // This crate trusts that `candidates` already contains distinct
        // client IDs (that's `conflux-registry`'s job upstream, not
        // this one's) — it doesn't itself detect or collapse repeats.
        // Documented here so the assumption is explicit rather than
        // silently relied on: a duplicated ID in the input can come back
        // selected twice.
        let selector = UniformRandomSelector {
            seed: SelectionSeed::Fixed(3),
        };
        let pool = vec!["client-0".to_string(); 5];

        let selected = selector.select(&pool, 3, 0);

        assert_eq!(selected.len(), 3);
        assert!(selected.iter().all(|c| c == "client-0"));
    }

    #[test]
    fn os_random_selects_the_right_count() {
        let selector = UniformRandomSelector {
            seed: SelectionSeed::OsRandom,
        };
        let pool = candidates(20);

        assert_eq!(selector.select(&pool, 5, 0).len(), 5);
    }

    #[test]
    fn build_selector_succeeds_for_uniform_random() {
        assert!(build_selector("uniform_random", SelectionSeed::Fixed(1)).is_ok());
    }

    #[test]
    fn build_selector_fails_for_an_unknown_name() {
        // `Box<dyn ClientSelector>` isn't `Debug`, so `.unwrap_err()`
        // isn't usable here — match directly, same reasoning as
        // `conflux-core`'s analogous test.
        match build_selector("does_not_exist", SelectionSeed::Fixed(1)) {
            Err(SelectorBuildError::Unknown(name)) => assert_eq!(name, "does_not_exist"),
            Ok(_) => panic!("expected an error, got a constructed ClientSelector"),
        }
    }

    #[test]
    fn every_buildable_name_is_also_registry_visible() {
        assert!(build_selector("uniform_random", SelectionSeed::Fixed(1)).is_ok());
        assert!(conflux_config::lookup(StrategyKind::Selector, "uniform_random").is_some());
    }
}
