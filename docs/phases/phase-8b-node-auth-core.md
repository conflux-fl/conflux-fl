# Phase 8b — Node authentication: config + allow-list core

## Scope
The data model and config knob for real node authentication — closing
gap 2 from `docs/FLOWER_COMPARISON.md` ("registration has no cryptographic
identity check"). This phase builds the allow-list itself and the mode-
driven on/off toggle; enforcement at the `register()` RPC and the admin
surface to populate the allow-list are Phase 8c.

**The on/off requirement, explicitly**: node auth must be skippable for
research/fast-iteration, the same way `allow_stub_client` already lets
research mode skip the real-`ClientApp` requirement. This isn't a
research-only convenience bolted on after the fact — it's the same
mode-owned-parameter-with-explicit-override shape spec §4.1 already uses
everywhere else, so it's built in from this phase, not retrofitted later.

## Inputs
- `conflux-config`'s mode-owned parameter pattern (`allow_stub_client` is
  the closest precedent: research default `true`/off-by-default-safety,
  production default the stricter value) — `require_node_auth` follows the
  identical shape (research default `false`, production default `true`).
- `conflux-registry::{ClientId, RegistryError}` (Phase 1) and the
  `Registry`/backend-enum pattern Phase 8a just established — the
  allow-list gets the same treatment (a trait, an in-memory impl, a Redis
  impl, an `Any*` enum) for the same reasons.
- Spec §7's node-identity story: Flower's `flwr supernode register` proves
  a real, pre-approved keypair; this phase's `NodeIdentity` is Conflux's
  equivalent data shape, deliberately supporting *two* proof mechanisms
  (see Deliverables) so node auth doesn't force mTLS to also be enabled —
  the two are independently toggleable.

## Deliverables
- `conflux-config`: new mode-owned parameter `require_node_auth: bool`.
  `ModeDefaults`/`Overrides`/`ResolvedConfig` gain the field;
  `Mode::Research::defaults()` sets `false`, `Mode::Production::defaults()`
  sets `true`; `to_log_lines()` includes it (ADR 0007 — this is a security
  posture, it must be visible in the startup log like everything else);
  `resolve()`'s existing precedence chain applies unchanged (an explicit
  override still wins over the mode default in either direction).
- `conflux-registry::NodeIdentity`: `CertFingerprint(String)` (SHA-256 hex
  of the DER-encoded peer certificate, when mTLS is in use) |
  `SharedToken(String)` (a pre-shared secret, when it isn't) — either
  proof mechanism works independently of whether mTLS (Phase 7e) is also
  turned on for a given deployment.
- `conflux-registry::NodeAllowlist` trait: `allow(client_id, identity)`,
  `revoke(client_id)`, `check(client_id, presented) -> bool`,
  `list() -> Vec<ClientId>`. `NodeAuthError` (thiserror): `NotAllowed`,
  `Backend(String)`.
- `InMemoryNodeAllowlist` (research default — ephemeral is fine, since
  `require_node_auth` defaults off in research anyway) and
  `RedisNodeAllowlist` (production — durable, same Redis container as
  `RedisRegistry`, different key namespace).
- `AnyNodeAllowlist` enum (`InMemory | Redis`), same delegation pattern as
  Phase 8a's `AnyRegistry`/`AnyStore`.

## Test plan
- `conflux-config`: `require_node_auth` resolves `false` for research,
  `true` for production, by default; an explicit override in either
  direction wins over the mode default (mirrors every existing
  `resolve()` precedence test).
- `InMemoryNodeAllowlist`/`RedisNodeAllowlist`: `allow` then `check` with
  the matching identity passes; `check` with a *different* identity for
  the same `client_id` fails (proves this isn't just "is the id present,"
  it's "does the presented proof match"); `check` for a never-allowed
  `client_id` fails; `revoke` then `check` fails even with the originally-
  correct identity; `list` reflects current membership. `RedisNodeAllowlist`
  runs against the real dev Redis container, same as `RedisRegistry`'s
  own tests.

## Definition of done
- [x] `cargo test -p conflux-config -p conflux-registry` passes, including
      `RedisNodeAllowlist`'s tests against real Redis.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.

## Outcome

`require_node_auth: bool` added to `conflux-config` following
`allow_stub_client`'s exact shape: `ModeDefaults`, both `Mode::defaults()`
arms (research `false`, production `true`), `Overrides`, `ResolvedConfig`,
`resolve()`'s layer call (env var `CONFLUX_REQUIRE_NODE_AUTH`), and
`to_log_lines()`. Two new tests confirm the mode default in both
directions and that an explicit override wins.

`conflux-registry` gained `node_allowlist.rs` (`NodeIdentity`,
`NodeAllowlist` trait, `NodeAuthError`, `InMemoryNodeAllowlist`),
`redis_node_allowlist.rs` (`RedisNodeAllowlist`, storing entries in one
Redis hash keyed by client id, values tagged `cert:`/`token:`), and
`any_node_allowlist.rs` (`AnyNodeAllowlist`, same enum-delegation pattern
as `AnyRegistry`/`AnyStore`). `check` returns `Result<bool, NodeAuthError>`
rather than a bare `bool` (deviating slightly from the brief's literal
signature) — a real backend can fail to answer at all (Redis down), and
that needs to be distinguishable from a genuine "not allowed" for Phase
8c's enforcement to react correctly to each.

All test-plan cases covered: matching identity passes; wrong identity for
a real client fails; unknown client fails; revoke-then-check fails even
with the original identity; `list` reflects membership; and (added beyond
the brief) a `CertFingerprint` and a `SharedToken` carrying the same raw
string are distinct, not accidentally equal.

122 tests passing workspace-wide (was 105 at the end of Phase 8a), stable
across repeated runs; `cargo fmt --check` and
`cargo clippy --workspace --all-targets` both clean.
