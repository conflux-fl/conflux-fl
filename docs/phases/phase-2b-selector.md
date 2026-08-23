# Phase 2b — `conflux-selector`

## Scope
Client sampling strategies. Ships `UniformRandomSelector`, the one cited
implementation from spec §5 (McMahan et al., 2017). Does **not** build any
resource-aware or utility-based selector (future/Phase 8, per spec §3's
`edge` row), and does not depend on `conflux-registry`'s `ClientId` — per
spec §2's dependency graph, `conflux-selector` has no internal crate
dependency, so candidates are addressed by plain `String` here; wiring a
concrete client-id type through is `conflux-server`'s job in Phase 5.

## Inputs
- Spec §5: `pub struct UniformRandomSelector;`, cited to McMahan, Moore,
  Ramage, Hampson & y Arcas (2017), *Communication-Efficient Learning of
  Deep Networks from Decentralized Data*, AISTATS.
- Spec §5: "Seeding is configurable (§8: `seed_mode`), defaulting
  `fixed(42)` in research, `os_random` in production — the latter matters
  specifically for crowdsourcing, where a predictable seed could let an
  adversary anticipate client selection."
- `conflux-config`'s `SeedMode`/`seed_value` resolution (Phase 1, already
  built) is what a caller will feed into this crate's seed choice —
  `conflux-selector` itself doesn't read config, it just accepts a seed.

## Deliverables
- `ClientSelector` trait: `select(&self, candidates: &[String], n: usize,
  round: u64) -> Vec<String>`. `round` lets a fixed-seed selector vary its
  output deterministically per round without needing interior mutability
  (no `Mutex`-guarded RNG state) — round 1 and round 2 don't select
  identical subsets, and re-running round 5 twice gives the same subset
  both times.
- `SelectionSeed` enum: `Fixed(u64) | OsRandom`.
- `UniformRandomSelector { seed: SelectionSeed }` — samples `n` candidates
  without replacement; returns all candidates if `n >= candidates.len()`.

## Test plan
- Fixed seed: two selectors with the same `Fixed(seed)` and same `round`
  produce identical output; the same selector with different `round`
  values produces different output (probabilistically, but verify at least
  one differs across a handful of rounds); `OsRandom` selectors are not
  required to be reproducible.
- `n >= candidates.len()` returns every candidate, no panic.
- `n == 0` returns an empty vec.
- No duplicates in the selection (sampling is without replacement).

## Definition of done
- [x] `cargo test -p conflux-selector` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.
