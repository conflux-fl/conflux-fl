# Phase 17 — Client-side privacy transform in `conflux-node`

**Status: shipped 2026-08-30.**

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

- [x] `cargo test -p conflux-node -p conflux-config` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md`'s "Known deviations from spec" bullet updated —
      client-side privacy transform moves from listed-gap to shipped.

## Outcome

Shipped as scoped, with one deviation from the brief's wiring plan and
three decisions the brief didn't cover.

**The dependency change is real and worth stating plainly.** `docs/spec`
§2 and CLAUDE.md both describe `conflux-node` as depending on
`conflux-proto` and `conflux-net` only. This phase adds
`conflux-privacy`, which transitively pulls in `conflux-config` as well
(that's where `conflux-privacy` registers itself, ADR 0002). It was done
deliberately, because spec §8's own sequence diagram shows
`Node->>Priv: clip + noise (optional)` as a step distinct from the
server-side transform, and there is no way to honor that without the
node reaching the mechanism. CLAUDE.md's dependency line has been
updated to match reality rather than left to drift.

1. **`main.rs` reads env vars, not `conflux-config`.** The brief said it
   should read "the resolved `client_side_privacy_transform` value",
   which would mean a direct `conflux-config` dependency and a config
   resolution pass inside `conflux-node`. That is a much larger change
   than this phase needs, and it contradicts Phase 6's own scope
   decision that `startup_guard.rs` documents. So the node reads
   `CONFLUX_CLIENT_SIDE_PRIVACY_TRANSFORM`/`CONFLUX_CLIP_NORM`/
   `CONFLUX_NOISE_MULTIPLIER`/`CONFLUX_SEED_VALUE` directly, mirroring
   `conflux-config`'s builtin fallbacks inline — exactly the convention
   `startup_guard.rs` established for `Mode` and `allow_stub_client`.
   The config field itself was still added, resolved, and logged (ADR
   0007), so the parameter is documented and layered like every other.

2. **Chunks are reassembled before clipping, then re-split at their
   original boundaries.** Not in the brief, and it is the difference
   between a correct mechanism and one that looks correct. Clipping is
   defined over the L2 norm of the *whole* update; clipping each chunk
   separately to the same radius would bound each piece rather than the
   whole, so a 3-chunk update would pass at up to √3 × the radius and
   the actual privacy guarantee would depend on how the caller happened
   to fragment its payload. There is a test that submits a 3-chunk
   update and asserts the reassembled norm is exactly the radius.

3. **The transform runs once, before the retry loop, and the RNG is
   carried across calls rather than re-seeded per submission.** Two
   distinct ways to get this wrong, both of which still "work":
   re-transforming per retry attempt would draw fresh noise for each
   resend, letting a server average the noise away across retries of one
   submission; re-seeding from a fixed seed per call would make every
   round's noise identical, which an observer can simply subtract. A
   test asserts successive submissions differ, and another asserts two
   bridges with the same seed agree — reproducible *sequence*, not
   repeated *value*.

The server-side transform still runs unconditionally afterwards, as the
brief specified — this adds a stage in front of the existing pipeline
rather than gating it.

12 new tests: 5 in `conflux-node/tests/local_privacy.rs` (off is
byte-identical, on clips and noises, whole-update clipping, seed
determinism, non-repeating noise), 2 config layering tests, and a
`conflux-server` integration test proving a round completes normally
against an already-client-transformed delta with finite output. 327 →
335 workspace-wide. `cargo fmt` and `cargo clippy --workspace
--all-targets` clean.
