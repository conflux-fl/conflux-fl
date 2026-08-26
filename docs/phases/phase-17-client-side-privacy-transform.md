# Phase 17 (draft) — Client-side privacy transform in `conflux-node`

**Status: scoping draft, not started.**

## Scope

Spec §8's sequence diagram shows `Node->>Priv: clip + noise (optional)`
immediately after the Python `ClientApp` returns a trained delta, and
*separately*, `Server->>Priv: transform_client_delta()` server-side after
`conflux-buffer` flushes a round's batch. Today only the server-side call
exists — `conflux-node` has no dependency on `conflux-privacy` at all
(`crates/conflux-node/Cargo.toml` confirmed). This phase adds the
client-side half: an optional local DP transform applied to a client's
own delta *before* it leaves `conflux-node`, independent of whatever
server-side transform also runs.

**Why both can coexist without double-transforming incorrectly**: the two
apply at different trust boundaries and answer different threat models.
Client-side DP protects a client's raw update from ever being observable
in the clear by the network/server at all (relevant for
`crowdsource`/`edge` topologies where the server isn't fully trusted);
server-side DP (already shipped) protects the aggregate's exposure once
batched. Running both is a legitimate, common deployment choice in the
literature (local DP + a server-side accounting layer on top) — this
phase doesn't decide a deployer must pick one; it makes the client-side
option real so that choice becomes possible at all. `conflux-privacy`'s
epsilon *accounting* (`RdpAccountant`) stays server-side only, unchanged
by this phase (ADR 0006's `Global` scope already accounts for whatever
noise arrives at the server, regardless of where it was added) — this
phase adds a second **mechanism** application point, not a second
accounting scope.

## Inputs

- `conflux-privacy::GaussianClippingPrivacy::transform(&self, weights:
  &mut [f32], rng: &mut dyn rand::Rng)` — already a pure, stateless
  function over a mutable weights slice; nothing about its current
  implementation is server-only, so it's directly callable from
  `conflux-node` once that crate depends on `conflux-privacy`. No change
  needed to `conflux-privacy` itself.
- `conflux-node::bridge.rs`'s `NodeBridge` — the point in the pipeline
  where a trained delta (received from the Python `ClientApp` over the
  local gRPC hop) is about to be forwarded to `submit_delta`. This is
  where the transform call belongs: after the delta is decoded from the
  local hop's `DeltaChunk`s, before it's re-encoded for the network hop
  to `conflux-server`.
- `conflux-config`'s existing `privacy_mechanism`/`clip_norm`/
  `noise_multiplier`/`target_epsilon` fields — reused as-is for
  *parameters*, gated by one new boolean deciding *where* the transform
  runs.

## Design decision this brief makes explicit

A new config field, `client_side_privacy_transform: bool` (builtin
fallback `false` — matches every other opt-in security/privacy posture
in this codebase, e.g. `reputation_filter_enabled`), independent of
whether `privacy_mechanism` is configured at all. When `true`,
`conflux-node` applies `GaussianClippingPrivacy::transform` (built from
the same resolved `clip_norm`/`noise_multiplier` the server-side path
already uses) to the delta before submission; the server-side transform
still runs afterward, unconditionally, exactly as today — this phase
adds a client-side stage in front of the existing pipeline, it doesn't
remove or gate the server-side one. A deployer wanting client-side-only
noise would need a separate follow-on (making the server-side transform
itself gateable) — explicitly **not** in this phase's scope, to keep this
change additive and low-risk rather than restructuring the existing,
working server-side path.

## Deliverables

- `crates/conflux-node/Cargo.toml`: adds `conflux-privacy` and `rand` as
  dependencies.
- `conflux-config`: new field `client_side_privacy_transform: Option<bool>`
  on `Overrides`, builtin fallback `false`, resolved and logged (ADR
  0007) like every other boolean toggle in this codebase.
- `conflux-node::bridge.rs`: `NodeBridge` gains an
  `Option<GaussianClippingPrivacy>` field, constructed at startup from
  the resolved config (`None` when the toggle is off — zero runtime cost
  for the common case). In `submit_delta`, before forwarding to
  `upstream.submit_delta`, decode the weights, call `.transform(&mut
  weights, &mut rng)` if present, re-encode. `rng` is a per-`NodeBridge`
  `StdRng` seeded at construction (deterministic-if-seeded, matching
  `conflux-config`'s existing `seed_mode`/`seed_value` research-
  reproducibility convention) rather than re-seeding per call.
- `main.rs` (`conflux-node`'s binary entry point): reads the resolved
  `client_side_privacy_transform` value and constructs `NodeBridge`
  accordingly — no new env var beyond what `conflux-config`'s existing
  layering already provides (this is an ordinary resolved config value,
  not connection material like Phase 9a/16's cert/key paths).

## Test plan

- `NodeBridge` unit test: with the toggle off, a submitted delta is
  byte-identical to what the Python `ClientApp` returned (zero
  transformation — proves the default path has no behavior change).
- With the toggle on: a submitted delta differs from the raw trained
  delta (noise was added), and its L2 norm is bounded by `clip_norm`
  post-clipping (before noise) — same invariant `conflux-privacy`'s own
  existing `GaussianClippingPrivacy` tests already check server-side,
  now exercised from the client-side call site.
- End-to-end (extending the existing Phase 6 pull-mode E2E test): a full
  round with `client_side_privacy_transform = true` and the stub Python
  `ClientApp` completes normally — the server-side aggregation still
  succeeds against noised-and-clipped-twice deltas (proves the two
  transform stages compose without erroring, not that the resulting
  model quality is good — that's a research question, not this phase's
  correctness bar).
- Determinism: two `NodeBridge` instances constructed with the same
  `seed_value` produce identical noised output for identical input —
  matching this codebase's existing reproducibility guarantees elsewhere.

## Definition of done

- [ ] `cargo test -p conflux-node -p conflux-config` passes.
- [ ] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [ ] `docs/STATUS.md`'s "Known deviations from spec" bullet updated —
      client-side privacy transform moves from listed-gap to shipped.
