//! Client sampling strategies.
//!
//! See `docs/spec/conflux-spec-v1.md` §5.

use conflux_config::{StrategyEntry, StrategyKind};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;

/// How `UniformRandomSelector` seeds its RNG — mirrors `conflux-config`'s
/// `SeedMode`, but this crate has no dependency on `conflux-config` (spec
/// §2's dependency graph lists no such edge), so the caller translates
/// `SeedMode`/`seed_value` into this enum.
#[derive(Debug, Clone, Copy)]
pub enum SelectionSeed {
    /// Deterministic, combined with the round number so consecutive
    /// rounds don't select an identical subset.
    Fixed(u64),
    /// Seeded from OS entropy on every call; not reproducible. Spec §5:
    /// matters specifically for crowdsourcing, where a predictable seed
    /// could let an adversary anticipate client selection.
    OsRandom,
}

/// Selects up to `n` clients from `candidates` for one round.
///
/// `&self` (not `&mut self`), plus an explicit `round` parameter, rather
/// than a `Mutex`-guarded RNG field: every implementation stays trivially
/// `Send + Sync`, and a fixed-seed selection is reproducible independent
/// of call order — re-running round 5 always gives the same subset.
pub trait ClientSelector: Send + Sync {
    fn select(&self, candidates: &[String], n: usize, round: u64) -> Vec<String>;
}

/// The one shipped selector (spec §5), cited to McMahan, Moore, Ramage,
/// Hampson & y Arcas (2017), *Communication-Efficient Learning of Deep
/// Networks from Decentralized Data*, AISTATS — the client-sampling
/// strategy from the original FedAvg algorithm.
pub struct UniformRandomSelector {
    pub seed: SelectionSeed,
}

// Phase 10b: registers this family's one member into `conflux-config`'s
// compile-time strategy registry (ADR 0002) — see `conflux-core`'s
// analogous `inventory::submit!` for `Aggregator`s for the full reasoning.
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Selector, name: "uniform_random" }
}

#[derive(Debug, thiserror::Error)]
pub enum SelectorBuildError {
    #[error(
        "unknown selector \"{0}\" — not a registered conflux-selector strategy \
         (known: \"uniform_random\")"
    )]
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
