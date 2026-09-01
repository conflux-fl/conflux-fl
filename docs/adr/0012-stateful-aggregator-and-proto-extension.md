# 0012 — Cross-round aggregator state and per-client extra fields: FedNova/SCAFFOLD/FedOpt

**Status: accepted and implemented (2026-08-31).** Approved by the
project owner; both extensions have shipped. Three things had to be
decided during implementation that this document did not settle — they
are recorded in "Corrections found while implementing" at the end rather
than silently folded into the text above, so the difference between what
was decided and what was discovered stays visible.

Originally scoped the
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
  uint64 num_samples = 4;                // uint64, not uint32 — see Correction 1
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
- FedNova becomes buildable once `local_steps` is populated by at least
  one real client path. ~~As a straightforward `AveragingWeighting`
  member (`StepNormalizedWeighting`, per
  `AGGREGATION_LANDSCAPE.md`).~~ **Corrected 2026-09-01 by building
  it:** FedNova is *not* a weighting. Its update leaves an `x_t` term
  that vanishes only when `τ_eff·Σ(p_k/τ_k) = 1`, which by
  Cauchy–Schwarz holds iff every `τ_k` is equal — exactly when FedNova
  reduces to FedAvg. It needs cross-round state, and uses this ADR's own
  `Mutex` pattern. The prediction was wrong about the *shape*; the
  plumbing this ADR added was still what unblocked it.
- SCAFFOLD and FedOpt still each need their own dedicated phase brief
  after this ADR lands — this document unblocks the plumbing, it doesn't
  scope either method's own algorithm.

## Corrections found while implementing (2026-08-31)

Three things this ADR got wrong or left open. All were found by building
it, none by re-reading it.

### 1. `DeltaChunk` needed the fields too — `ClientDelta` never travels

The snippet above extends `ClientDelta` only. But `ClientDelta` is not a
wire message: it is what `conflux-server` *builds* by reassembling a
client's `DeltaChunk` stream, as its own schema comment has always said
("Never sent on the wire as-is"). Fields added only to `ClientDelta`
could therefore never be populated by any client — the extension would
have compiled, tested green against hand-built `ClientDelta`s, and
carried nothing.

Both fields are on `DeltaChunk` as well (7 and 8), and they reassemble
differently because they are differently shaped:

- **`local_steps`** is a scalar, so it follows `num_samples`'s existing
  convention exactly: repeated on every chunk, read from whichever chunk
  arrives *first*. Depending on chunk 0 arriving first would be
  depending on network ordering.
- **`control_variate`** is a full vector, so it is chunked exactly like
  `data` — chunk *i* carries slice *i* — and concatenated in
  `chunk_index` order, not arrival order. These two rules are different,
  and a test that only ever submitted in-order chunks would pass with
  them confused, so every reassembly test submits out of order.

(The snippet also typed `num_samples` as `uint32`; the real schema has
always used `uint64`. Corrected above rather than propagated.)

### 2. `max_update_bytes` had to learn about the new field

Tier 5's H1 bound counted `chunk.data.len()` and nothing else, because
`data` was the only client-controlled payload field when it was written.
`control_variate` is a second one. Left alone, the fix would still have
*existed* and been trivially bypassable: put the flood in
`control_variate`, keep `data` tiny, and allocate exactly as much server
memory as before while every counted byte stays near zero.

The bound now sums both, and
`conflux-net/tests/update_size_limit.rs::the_limit_counts_control_variate_bytes_not_just_data`
was confirmed to fail with the old accounting restored. **Any future
payload field must be added to that sum** — a ceiling a client can step
around by choosing a different field is not a ceiling. Recorded in
`docs/EXTENDING.md` as one of the three edits a new field requires.

### 3. "No existing producer needs to change" is true of bytes, not of Rust

The Consequences section says no existing `ClientDelta` producer needs to
change. That is exactly right *on the wire* — and it is now proven at the
byte level rather than asserted: a delta with both fields absent encodes
byte-for-byte identically to what the old schema produced, checked
against a hand-built expected encoding rather than against the type under
test.

It is not true of Rust source. Adding a field to a `prost` struct breaks
every literal that names fields exhaustively — 75 of them, across 37
files in this workspace. They now end in `..Default::default()`, which is
what makes the claim true *going forward*: the next optional field will
break none of them. New code should follow that idiom for the same
reason.

> **Caveat added 2026-09-01, after this advice caused a defect.**
> `..Default::default()` is right for a literal that *constructs* a value
> — a test fixture, a builder. It is **wrong** for one that *transforms*
> an existing value, because there it silently resets every field the
> transform does not name. `conflux-server`'s `reencode_passing_deltas`
> rebuilt each `ClientDelta` after reputation filtering and ended in
> `..Default::default()`, which reset all three of this ADR's fields to
> `None` on the last hop before `aggregate`. q-FedAvg found no
> `local_loss` and silently ran as FedAvg; FedNova and SCAFFOLD would
> have been dead on arrival too. Nothing failed — the configured method
> just never ran, which is why it survived every unit test on both sides
> of that function. **In a pass-through, name every field explicitly and
> let the compiler break the next person who adds one.**

### What is now unblocked, and what is not

The plumbing is in place and tested. **No aggregator reads either field
yet** — FedNova, SCAFFOLD, and FedOpt each still need their own phase
brief, exactly as this ADR's "What this ADR does *not* decide" section
says. What has changed is that none of them is blocked on a schema
decision any more, and the obligations that come with statefulness are
written down (`docs/EXTENDING.md`, and the `Aggregator::aggregate` doc
comment) rather than left to be rediscovered.
