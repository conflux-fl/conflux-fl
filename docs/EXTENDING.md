# Extending Conflux

How to add a new aggregation method, client selector, privacy mechanism,
or attack, without touching `conflux-server`. All four follow the same
shape (ADR 0002's family pattern, extended by/11b's registry
wiring): a small trait impl in the owning crate, one `inventory::submit!`,
one match arm in that crate's `build_*` function. `conflux-server` never
learns a new name exists — it just reads whatever `config.aggregator.value`
resolves to and asks the registry for it.

If you're looking for the *design rationale* behind this pattern, see
ADR 0002 and ADR 0008
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
// including another robust aggregator (see its own test,
// `filtered_aggregator_composes_with_a_non_fedavg_combiner`, for a
// worked example of composing a filter with a non-FedAvg combiner).
```

**C. Coordinate-wise robust variant** (like Trimmed Mean/Median — "for
each weight-vector index independently, combine every client's value at
that index"): implement `CoordinateWiseRobustStatistic`. Use this shape,
not (B), when your method doesn't select whole client updates — Phase
11a's own rationale (its phase brief) for
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

### If your method needs memory across rounds (ADR 0012)

Some published methods cannot work from a single batch: FedOpt keeps
moment estimates, Centered Clipping keeps a running reference, FoolsGold
keeps per-client history. **Hold that state in a `Mutex` field on your
aggregator. Do not reach for `&mut self`.**

```rust
pub struct MyStatefulAggregator {
 // `Mutex` because `aggregate` takes `&self`: one aggregator serves
 // every round behind an `Arc`, so interior mutability is what lets a
 // method carry memory without changing the trait for the twelve
 // methods that don't need any.
 history: std::sync::Mutex<HashMap<String, Vec<f32>>>,
}
```

Why not `&mut self`: it would force every *existing* stateless
aggregator behind a `Box<dyn Aggregator>` to be called through exclusive
access as well, which is a change to `conflux-server`'s whole round
pipeline, in exchange for a capability a minority of methods need. Four
shipped methods already use the `Mutex` pattern — `FoolsGoldAggregator`,
`CenteredClippingAggregator`, `DssAggregator`, and whatever you are
about to write — so copy one of them rather than inventing a third
approach.

**Two obligations come with statefulness**, both of which Tier 6 found
the hard way:

1. **Validate what you store, not only what you receive.**
 `decode_and_validate` guards the batch in front of you. Nothing
 re-checks the reference or history you derived from an earlier batch.
 A single finite, validation-passing update drove Centered Clipping's
 stored reference to `NaN` — and because that reference is what every
 later round clips against, no honest round afterwards could recover.
2. **Add cross-round tests.** `tests/adversarial_input.rs` hands each
 aggregator one batch and cannot express "round N poisons round N+1",
 which is the whole failure class you have just opted into. Add your
 method to `tests/stateful_adversarial_input.rs` instead — it submits
 sequences, and it exists because four real defects were living in
 that gap.

### If your method needs a field `ClientDelta` doesn't have (ADR 0012)

FedNova needs each client's local step count; SCAFFOLD needs a full
control-variate vector. Both already exist on the wire as **optional**
fields:

| Field | Type | Shape |
|---|---|---|
| `local_steps` | `Option<u32>` | scalar — a client repeats it on every chunk, and the server reads it from whichever chunk arrives first |
| `control_variate` | `Option<Vec<u8>>` | vector — chunked exactly like `weights`, concatenated in `chunk_index` order, same little-endian f32 codec |

Read them straight off `ClientDelta`. `None` means "this client is not
running your method", which is deliberately distinct from `Some(0)` or
`Some(vec![])`.

If you read `control_variate`, **check its decoded length against
`weights`' length and reject a mismatch**. The transport deliberately
does not: `conflux-server` is opaque to model architecture (ADR 0004), so
it has no basis for knowing what length is correct, and a client that
populated the field on only some of its chunks produces a short vector
that reaches you intact rather than being silently padded.

Adding a *third* such field is the same three edits — the `.proto`
message pair, the reassembly in `conflux-server`'s `submit_delta`, and
the byte count in `conflux-net`'s `submit_delta` (any client-controlled
payload field must count toward `max_update_bytes`, or the bound simply
moves aside). Construct `ClientDelta`/`DeltaChunk` literals with
`..Default::default()` so the next addition doesn't break yours.

### If your method needs a signal the server computes (ADR 0011)

FLTrust and Zeno score clients against something the *server* produces
from its own data, not against anything in the batch. That needs a
training capability `conflux-server` deliberately does not have (ADR
0004), so it lives in the optional `conflux-trusted-reference` sidecar.

To add another such method:

1. Implement `Aggregator` as usual, and override two defaulted methods:

 ```rust
 fn requires_trusted_reference(&self) -> bool { true }
 fn set_trusted_reference(&self, r: TrustedReference) { /* store it */ }
 ```

 The round pipeline calls the first to decide whether to contact a
 sidecar at all, then the second before `aggregate`. Store what you are
 given behind a `Mutex`, per ADR 0012.

2. **Refuse to run without it.** Return
 `AggregatorError::MissingTrustedReference` rather than falling back to
 anything. The obvious fallback — an unweighted mean — is FedAvg, which
 is the method these exist to replace, and substituting it silently at
 the moment the defense should engage produces a checkpoint that looks
 healthy and is not.

3. If you need a signal the sidecar does not already serve, add an RPC to
 `conflux-proto/proto/trusted_reference.proto` and a method to the
 `TrustedModel` trait. `ScoreUpdates` already exists and is unused by
 any aggregator — Zeno's, waiting for Zeno.

**Do not add `conflux-trusted-reference` as a dependency of anything
`conflux-server` builds.** The server calls a sidecar over gRPC using the
client in `conflux-net`; it must never *be* one, or the model-runtime
dependency ADR 0004 keeps out of the server comes back in through the
side door. CI's `isolation` job fails the build if that edge appears.

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
defenses, ADR 0008) — see its phase brief's
table for the existing four attacks' citations as examples of the
expected level of detail (paper, venue, year, and — where relevant, like
`ScalingAttack`'s — an explicit note on what's simplified relative to
the source paper's full scope).

## What doesn't need a registry entry

Backend selection (`RegistryBackend`/`StoreBackend`/`AccountingBackend`
in `conflux-server::backend_selection`) is deliberately **not** part of
this pattern — a Redis URL is a deployment detail, not an
experiment-tuning algorithm choice, so it stays env-var-driven
(its phase brief's scope note explains the
distinction). Don't add a new backend as a registry `StrategyEntry`; add
it to `BackendSelection`'s own enums instead, following that phase's
precedent.
