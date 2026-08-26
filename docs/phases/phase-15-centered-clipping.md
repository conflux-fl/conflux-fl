# Phase 15 (draft) — Centered Clipping (CClip)

**Status: scoping draft, not started.**

## Scope

Implement Centered Clipping (Karimireddy, He & Jaggi, 2021 — "Learning
from History for Byzantine Robust Optimization") as a new `conflux-core`
family member. Per `docs/AGGREGATION_LANDSCAPE.md` Category 2/4, this is
the one already-tracked method that's both a robust-aggregation method
*and* needs cross-round state — a `temporal.rs`-shaped aggregator
(`temporal.rs`'s `Mutex`-based per-client history pattern, proven by
FoolsGold and DSS, is the direct precedent), not a new `averaging`/
`robust` trait member. Unlike FedNova/SCAFFOLD (`docs/adr/
0012-stateful-aggregator-and-proto-extension.md`), Centered Clipping
needs **no `conflux-proto` change** — every input it needs is already on
the wire (`weights`) — so it's buildable independently of that ADR
landing, and independently of Phase 14.

## The algorithm, as published

Per round `t`, given a server-held running reference vector `v^(t)`
(persists across rounds, starts at the zero vector or the previous
round's aggregate — the paper uses momentum-style initialization):

1. For each client update `u_i`, compute `u_i' = v^(t) + min(1,
   τ / ‖u_i − v^(t)‖) · (u_i − v^(t))` — clip the *deviation from the
   reference*, not the raw update, to radius `τ`.
2. `v^(t+1) = v^(t) + (1/n) Σ_i u_i'` — the reference itself updates by
   the clipped average, which is also the round's aggregated output.

`τ` (clip radius) is the one tunable parameter, analogous to
`byzantine_fraction` for the `robust` family's existing members — a
config-resolved `f32`, not a hardcoded constant.

## Inputs

- `crates/conflux-core/src/temporal.rs` — `FoolsGoldAggregator`'s and
  `DssAggregator`'s existing `Mutex<...>`-based interior-mutability
  pattern for cross-round state under `Aggregator`'s unchanged `&self`
  signature (this is exactly the pattern `docs/adr/
  0012-stateful-aggregator-and-proto-extension.md` proposes formalizing
  as the standing answer for stateful family members — Centered Clipping
  is a second, independent real use case for it, strengthening that
  ADR's case regardless of whether SCAFFOLD/FedNova/FedOpt ever land).
- `l2_distance`/`l2_norm` helpers already in `temporal.rs` (built for
  DSS's own deviation-tracking) — directly reusable for CClip's own
  clip-radius distance calculation, no new vector-math helpers needed.
- `docs/AGGREGATION_LANDSCAPE.md`'s citation discipline (ADR 0008) — CClip
  must match Karimireddy et al.'s own formula exactly, including their
  specific choice of clipping the *deviation from the reference*
  (not the raw update against a fixed origin), which is what
  distinguishes it from a naive "clip every update to norm τ" scheme.

## Deliverables

- `CenteredClippingAggregator` in `temporal.rs`: `clip_radius: f32` field
  (config-resolved, same category as `robust_byzantine_fraction`) plus
  `reference: Mutex<Option<Vec<f32>>>` (starts `None`, meaning "use this
  round's plain mean as the initial reference," matching the paper's
  zero/warm-start initialization options — `None` is simpler to reason
  about than assuming a zero vector of unknown dimensionality before the
  first round's batch is seen).
- `inventory::submit!` registration as `"centered_clipping"` in
  `conflux-core::lib.rs`, wired into `build_aggregator` like every other
  shipped method (Phase 10b's registry pattern) — unlike `DssAggregator`,
  this is a literal, cited, standalone method (not a research hypothesis
  wrapping another method), so it belongs in the production catalog, not
  held out the way `docs/research/temporal-consistency-aggregation.md`'s
  `DssAggregator` deliberately is.
- New config field `clip_radius: Option<f32>` on `Overrides`/
  `ResolvedConfig`, same layering treatment as `robust_byzantine_fraction`
  (builtin fallback, no topology/mode ownership — an algorithm-tuning
  value, not a research/production posture).

## Test plan

- Hand-derived unit tests, same rigor as `FoolsGoldAggregator`'s
  hand-verified test: a small batch (3–4 clients, low-dimensional
  vectors) where the clip radius is deliberately set to bind for one
  outlier client — confirm that client's *contribution* is clipped to
  exactly the expected magnitude, not merely that the final result looks
  "reasonable."
- Reference persistence across rounds: two sequential `aggregate()` calls
  on the same instance, second round's clipping visibly centers on the
  first round's output (not on the raw batch mean) — the property that
  distinguishes this from a stateless robust method.
- Single-client and empty-batch edge cases, matching every other
  aggregator's existing test conventions (`AggregatorError::EmptyBatch`,
  single-client-returns-unchanged).
- Real-data evaluation: run `centered_clipping` through the exact
  Experiment 2.1/2.2 harness (`crates/conflux-attacks/examples/
  run_experiment.rs` — `centered_clipping` is a `--aggregator` value like
  any other shipped name once registered) against `ScalingAttack`/
  `PersistentSybilAttack`/`AdaptiveEvasionAttack`, to place it in
  `docs/research/temporal-consistency-aggregation.md`'s existing
  Category-6/temporal comparison alongside FoolsGold and DSS — CClip
  wasn't part of that research proposal's original scope, but its
  cross-round-state shape makes it directly comparable once built.

## Definition of done

- [ ] `cargo test -p conflux-core` passes, including CClip's hand-derived
      tests.
- [ ] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [ ] `docs/AGGREGATION_LANDSCAPE.md`'s summary table and `docs/STATUS.md`
      updated — CClip moves from "Not built" to shipped.
