# 0012 — Cross-round aggregator state and per-client extra fields: FedNova/SCAFFOLD/FedOpt

**Status: proposed — pending project-owner review.** Scopes the
architecture decision `docs/AGGREGATION_LANDSCAPE.md` Category 4 already
flagged as needed before any of these three methods could be built —
this ADR decides the shared plumbing question once, rather than each
method's eventual phase brief re-deriving it independently.

## Context

Three real, popular, cited methods are blocked on the same two
assumptions Conflux's current design bakes in, per
`docs/AGGREGATION_LANDSCAPE.md`'s Category 4 analysis:

- **`Aggregator::aggregate(&self, updates) -> Result<Vec<f32>, ...>`**
  (`crates/conflux-core/src/lib.rs`) takes `&self`, not `&mut self` —
  every family member is stateless across calls. `temporal.rs`
  (FoolsGold, DSS) already works around this via interior mutability
  (`Mutex<HashMap<...>>` fields) rather than a trait change — proof the
  workaround is viable, not proof a real capability exists. FedAdam/
  FedYogi/FedAdagrad ("FedOpt," Reddi et al., 2020) need genuine
  cross-round state: first/second-moment estimates of the aggregated
  delta, updated every round, the same *shape* of need `temporal.rs`
  already solved for a different purpose (Sybil-history tracking, not
  optimizer state).
- **`ClientDelta` carries only `weights` + `num_samples`** (spec §3).
  FedNova (Wang, Wang, Nusrat, Poor & Rajan, 2020) needs each client to
  also report its local step count, to normalize by (a `conflux-proto`
  schema change — one new scalar field). SCAFFOLD (Karimireddy, Kale,
  Mohri, Reddi, Stich & Suresh, 2020) needs each client to also send a
  full control-variate delta, same dimensionality as the model delta — a
  much bigger wire-format change, combined into the correction on both
  the client and server side.

None of these three is solvable as an ordinary `AveragingWeighting`/
`UpdateFilter`/`CoordinateWiseRobustStatistic` trait impl (ADR 0002's
existing shapes) — each needs plumbing changes to `Aggregator` itself
and/or `conflux-proto`, which is why `AGGREGATION_LANDSCAPE.md`
recommended deciding the shared shape *before* any one of the three gets
prioritized, rather than each landing its own bespoke, incompatible
extension.

## Decision

Two independent extensions, adopted together because both are needed
before any of the three methods is buildable, but scoped as genuinely
separable capabilities — a deployer using FedOpt never needs SCAFFOLD's
proto change, and vice versa:

### 1. Cross-round aggregator state becomes explicit, not a per-implementation workaround

`Aggregator::aggregate` keeps its current `&self` signature — `Mutex`-
based interior mutability (the `temporal.rs` pattern, already proven
correct by FoolsGold and DSS's own test suites) is adopted as the
**standing pattern** for any family member needing cross-round memory,
rather than changing the trait to `&mut self`. Rationale: `&mut self`
would force every *existing* stateless `Aggregator` behind a
`Box<dyn Aggregator>` to also be called through exclusive access (a
bigger blast-radius change to `conflux-server`'s round pipeline, which
currently treats `Box<dyn Aggregator>` as freely shareable), for a
capability only a minority of family members need. The `Mutex`-based
pattern is a smaller, purely additive change — any future stateful
method (a `FedOptWrapper`, Centered Clipping, or anything else) follows
`temporal.rs`'s precedent directly, no trait change required.

### 2. `ClientDelta` gains two new **optional** fields, not a breaking schema change

```protobuf
message ClientDelta {
  string client_id = 1;
  uint64 round = 2;
  bytes weights = 3;
  uint32 num_samples = 4;
  optional uint32 local_steps = 5;       // FedNova
  optional bytes control_variate = 6;    // SCAFFOLD — same encoding as `weights`
}
```

Both `optional` (proto3 `optional`, explicit presence) so every existing
client that doesn't set them is unaffected — `local_steps`/
`control_variate` absent means "this client isn't running FedNova/
SCAFFOLD," not zero or empty. `conflux-node`/the Python `ClientApp`
contract (ADR 0004, ADR 0005's still-deferred SDK) decides whether to
populate them; nothing server-side requires either field unless the
configured aggregator/selector actually reads it. This directly follows
FedNova's own path in `AGGREGATION_LANDSCAPE.md` ("needs a new field...
alongside the existing `num_samples`, so it's a `conflux-proto` schema
change, not just a Rust trait impl") — the same field-addition
mechanism serves SCAFFOLD too, since `bytes` already carries an
arbitrary-length encoded vector (the same encoding `weights` itself
uses, per `conflux-proto::encode_weights`/`decode_weights`), no second
codec needed.

## What this ADR does *not* decide

- **Whether to build FedNova, FedOpt, or SCAFFOLD at all** — each stays
  a separate future phase brief, gated on this ADR's plumbing landing
  first. This ADR only unblocks them; it doesn't prioritize them.
- **FedOpt's own trait shape** — whether it's a wrapping `Aggregator`
  (composing over any base, the way `DssAggregator` wraps any base
  `Aggregator` today — `temporal.rs` is directly reusable precedent) or
  a distinct post-aggregation pipeline stage `conflux-server` calls
  explicitly. The wrapping-aggregator shape is the more consistent
  choice given `DssAggregator`'s precedent already exists and required
  zero `conflux-server` pipeline changes to add — recommended, not
  decided here.
- **Centered Clipping's own trait shape** — tracked separately
  (`docs/phases/phase-15-centered-clipping.md`), since it needs cross-
  round state (this ADR's point 1) but no proto change (point 2) —
  narrower than FedNova/SCAFFOLD, buildable independently of this ADR's
  point 2 landing at all.

## Consequences

- No existing `ClientDelta` producer (real or stub `ClientApp`,
  `conflux-attacks`' `run_experiment`/`run_fairness_experiment`, every
  existing test fixture) needs to change — both new fields default to
  absent, and every current `Aggregator` ignores them by construction
  (they're not read anywhere yet).
- `temporal.rs`'s `Mutex`-based pattern is now the **documented,
  intentional** answer to "how does a family member hold cross-round
  state," not an ad hoc solution specific to FoolsGold/DSS — future
  contributors adding a stateful method have a named precedent to follow
  rather than reinventing the approach.
- FedNova becomes buildable as a straightforward `AveragingWeighting`
  member (`StepNormalizedWeighting`, per `AGGREGATION_LANDSCAPE.md`)
  once `local_steps` is populated by at least one real client path.
- SCAFFOLD and FedOpt still each need their own dedicated phase brief
  after this ADR lands — this document unblocks the plumbing, it doesn't
  scope either method's own algorithm.
