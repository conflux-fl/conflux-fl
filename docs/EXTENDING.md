# Extending Conflux

How to add a new aggregation method, client selector, privacy mechanism,
or attack, without touching `conflux-server`. All four follow the same
shape (ADR 0002's family pattern, extended by Phase 10b/11b's registry
wiring): a small trait impl in the owning crate, one `inventory::submit!`,
one match arm in that crate's `build_*` function. `conflux-server` never
learns a new name exists — it just reads whatever `config.aggregator.value`
resolves to and asks the registry for it.

If you're looking for the *design rationale* behind this pattern, see
[ADR 0002](adr/0002-family-pattern.md) and [ADR 0008](adr/0008-cited-baseline-implementations.md)
(citation discipline). This document is the practical "how," not the "why."

## The shared rule: cite your source

Every shipped method in this codebase is a literal, cited implementation
of a specific paper (ADR 0008) — not because the code has to be
published-paper-derived to be correct, but because an uncited "obvious"
default is how a codebase silently drifts from what it claims to
implement. If you're adding something genuinely novel (not from a paper),
say so explicitly in the doc comment instead of citing nothing — the
point is making the provenance visible, not gatekeeping originality.

## Adding a new aggregator

Two shapes exist today, both producing a `conflux_core::Aggregator`:

**A. Weighted-average variant** (like `FedAvg`, `FedAvgM`, inverse-loss
weighting — anything that's "combine every update with some per-update
weight"): implement `AveragingWeighting`.

```rust
// crates/conflux-core/src/averaging.rs
pub struct MyWeighting;
impl AveragingWeighting for MyWeighting {
    fn weight_for(&self, update: &ClientDelta, batch: &[ClientDelta]) -> f32 {
        // return a *relative* weight — WeightedAverageAggregator
        // normalizes across the batch for you.
    }
}
pub type MyAggregator = WeightedAverageAggregator<MyWeighting>;
```

**B. Selection-based robust variant** (like Krum/Multi-Krum — "score
every update, keep some subset, then combine the survivors"): implement
`UpdateFilter`, compose with any existing `Aggregator` as the combiner.

```rust
// crates/conflux-core/src/robust.rs
pub struct MyFilter { pub byzantine_fraction: f32 }
impl UpdateFilter for MyFilter {
    fn filter(&self, updates: &[ClientDelta]) -> Result<SelectionResult, AggregatorError> {
        // build a DistanceMatrix::from_updates(updates)? if you need one
        // (only filters that reason about pairwise distance do — a
        // filter that doesn't need one shouldn't pay for computing it),
        // score/select, return the indices you trust.
    }
}
// FilteredAggregator<MyFilter, FedAvg> combines your filter with
// existing sample-weighted averaging — or plug in a different combiner,
// including another robust aggregator (see phase-11a's own test,
// `filtered_aggregator_composes_with_a_non_fedavg_combiner`, for a
// worked example of composing a filter with a non-FedAvg combiner).
```

**C. Coordinate-wise robust variant** (like Trimmed Mean/Median — "for
each weight-vector index independently, combine every client's value at
that index"): implement `CoordinateWiseRobustStatistic`. Use this shape,
not (B), when your method doesn't select whole client updates — Phase
11a's own rationale (`docs/phases/phase-11a-robust-aggregation.md`) for
why these are genuinely different shapes, not just two implementations
of the same idea.

```rust
pub struct MyStatistic;
impl CoordinateWiseRobustStatistic for MyStatistic {
    fn combine(&self, values_at_one_coordinate: &mut [f32]) -> f32 {
        // sort in place if you need to, return one combined number
    }
}
// CoordinateWiseAggregator<MyStatistic> is your Aggregator.
```

**Then, regardless of shape**, in `crates/conflux-core/src/lib.rs`:

```rust
inventory::submit! {
    StrategyEntry { kind: StrategyKind::Aggregator, name: "my_aggregator" }
}
```

and add one arm to `build_aggregator`'s `match`. Add a test asserting
`build_aggregator("my_aggregator", ..)` is `Ok`, and that
`conflux_config::lookup(StrategyKind::Aggregator, "my_aggregator")` finds
it too (`every_buildable_name_is_also_registry_visible`'s existing shape
— extend that loop's `NAMES` constant, don't write a parallel test).
`aggregator = "my_aggregator"` in an experiment's `Overrides` now
resolves and constructs it — no `conflux-server` change.

If your method needs its own tunable parameter beyond
`robust_byzantine_fraction` (which every existing `robust` member
shares), add it to `conflux-config`'s `Overrides`/`ResolvedConfig`
following `robust_byzantine_fraction`'s own precedent (Overrides-only,
no topology/mode ownership, unless your parameter genuinely is a
research-vs-production posture rather than an algorithm-tuning value —
if it is, follow `require_node_auth`'s mode-owned precedent instead).

## Adding a new client selector

Same shape, in `conflux-selector`:

```rust
// crates/conflux-selector/src/lib.rs
pub struct MySelector { /* ... */ }
impl ClientSelector for MySelector {
    fn select(&self, candidates: &[String], n: usize, round: u64) -> Vec<String> {
        // pick up to n from candidates
    }
}

inventory::submit! {
    StrategyEntry { kind: StrategyKind::Selector, name: "my_selector" }
}
```

Add a `build_selector` match arm (it takes `seed: SelectionSeed` too, if
your selector wants reproducible-vs-OS-random behavior the way
`UniformRandomSelector` does — not every selector needs to; a
utility-based selector driven by real client metadata rather than
randomness can ignore the seed entirely). Extend the crate's own
`every_buildable_name_is_also_registry_visible`-shaped test.

## Adding a new privacy mechanism

Same shape, in `conflux-privacy`:

```rust
// crates/conflux-privacy/src/lib.rs
pub struct MyMechanism { /* your tunable fields */ }
impl PrivacyMechanism for MyMechanism {
    fn transform(&self, weights: &mut [f32], rng: &mut dyn rand::Rng) {
        // mutate weights in place — clip, add noise, whatever your
        // mechanism does. `&mut dyn Rng`, not a generic `impl Rng` —
        // required for `Box<dyn PrivacyMechanism>` to be constructible
        // (see `GaussianClippingPrivacy`'s own doc comment for why this
        // is a zero-cost, no-op change at every call site).
    }
}

inventory::submit! {
    StrategyEntry { kind: StrategyKind::PrivacyMechanism, name: "my_mechanism" }
}
```

`build_privacy_mechanism` takes `clip_norm`/`noise_multiplier` explicitly
today (the only mechanism that exists needs both) — if your mechanism
needs different or additional parameters, extend the function's
signature and update its one caller
(`conflux-server::app_state.rs::assemble`) accordingly; there's no
registry-level constraint forcing every mechanism to share the same
parameter shape, just today's one member's actual needs.

## Adding a new attack (`conflux-attacks`)

Different crate, same idea, deliberately **not** registry-wired — attacks
aren't a runtime-selectable strategy `conflux-server` ever picks by name;
they're test/dev-only code that must never become reachable from a
production binary (ADR 0010). Implement `Attack`:

```rust
// crates/conflux-attacks/src/attacks.rs
pub struct MyAttack { /* ... */ }
impl Attack for MyAttack {
    fn craft(&self, honest_updates: &[ClientDelta], num_attackers: usize) -> Vec<ClientDelta> {
        // "omniscient" model: you see the honest batch before crafting.
        // Use stats::{decode_all, coordinate_means, coordinate_std_devs}
        // if your attack needs them — most published attacks do.
    }
}
```

Add it to `crates/conflux-attacks/src/lib.rs`'s `pub use`. Then, in
`crates/conflux-attacks/tests/attack_vs_defense.rs`, add your attack to
the matrix against every shipped `Aggregator` — and **report what
actually happens, honestly**, the same discipline
`alie_attack_against_defended_aggregators_at_high_attacker_fraction`
already follows: if a defense doesn't hold against your attack in some
parameter regime, that's a real finding to document (in the test's own
assertions/output), not a test to loosen until it passes. Cite the
paper your attack comes from in its doc comment (same discipline as
defenses, ADR 0008) — see `docs/phases/phase-12-attack-simulation.md`'s
table for the existing four attacks' citations as examples of the
expected level of detail (paper, venue, year, and — where relevant, like
`ScalingAttack`'s — an explicit note on what's simplified relative to
the source paper's full scope).

## What doesn't need a registry entry

Backend selection (`RegistryBackend`/`StoreBackend`/`AccountingBackend`
in `conflux-server::backend_selection`) is deliberately **not** part of
this pattern — a Redis URL is a deployment detail, not an
experiment-tuning algorithm choice, so it stays env-var-driven
(`docs/phases/phase-8a-backend-selection.md`'s scope note explains the
distinction). Don't add a new backend as a registry `StrategyEntry`; add
it to `BackendSelection`'s own enums instead, following that phase's
precedent.
