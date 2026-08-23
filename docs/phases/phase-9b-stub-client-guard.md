# Phase 9b — Production stub-client guard

## Scope
Closes gap 5 from `docs/FLOWER_COMPARISON.md` / the CLAUDE.md constraint:
*"`conflux-server` must refuse to start in production
(`allow_stub_client = false`) without a real `ClientApp` connection
configured."* That wording names the wrong process — `conflux-server`
never talks to Python at all; only `conflux-node` has a local loopback
listener (ADR 0004) a `ClientApp` connects to. This phase implements the
guard where it architecturally belongs (`conflux-node`) and documents the
correction rather than silently reinterpreting the spec.

**What's actually enforceable today**: ADR 0005 defers the real Python
`ClientApp` SDK entirely — only `python/conflux_client/stub_client.py`
(fixed dummy weights, no PyTorch) exists. `conflux-node` has no protocol-
level way to distinguish a real `ClientApp` from the stub (no handshake
field carries that information, and inventing cryptographic proof of
"real training happened" is out of scope). The honest, implementable
guard is an explicit operator assertion: `conflux-node` refuses to start
in production unless the operator affirmatively declares what's listening
on the local loopback port, the same way `require_node_auth` (Phase 8b)
made a security posture an explicit, logged config value rather than an
implicit assumption.

## Inputs
- `crates/conflux-node/src/main.rs` — deliberately has no `conflux-config`
  dependency (`docs/phases/phase-6-node.md`'s scope note); this phase
  preserves that decision rather than reversing it, using plain env vars
  and locally-defined `RuntimeMode`/`ClientAppKind` enums instead of
  pulling in `conflux-config::Mode`/`Overrides`.
- `python/conflux_client/stub_client.py` and its README's existing
  human-facing "research-mode only" convention — this phase makes that
  convention machine-enforced.
- `conflux-server`'s `backend_selection.rs`/Phase 8b's `require_node_auth`
  — the precedent for a mode-driven fail-fast with an explicit override.

## Deliverables
- `conflux-node::startup_guard` (new module): `RuntimeMode { Research |
  Production }`, `ClientAppKind { Stub | Real }`, `StartupGuardError`
  (thiserror), and a pure function —
  `validate_client_app_startup(mode: RuntimeMode, allow_stub_client: bool, kind: ClientAppKind) -> Result<(), StartupGuardError>`
  — fails only when `mode = Production && !allow_stub_client && kind =
  Stub`; every other combination (research mode, an explicit
  `allow_stub_client` override, or an operator-declared `Real` kind)
  passes.
- `main.rs`: reads `CONFLUX_MODE` (mirrors `conflux-server`'s own
  main.rs parsing exactly — `"production"` vs. everything else),
  `CONFLUX_ALLOW_STUB_CLIENT` (explicit override; when unset, defaults
  from `CONFLUX_MODE` the same way `conflux-config`'s `allow_stub_client`
  mode-default does: `true` for research, `false` for production — kept
  as an inline default here rather than a new `conflux-config` dependency,
  per the Inputs note above), and `CONFLUX_CLIENT_APP_KIND` (`"stub"` |
  `"real"`, default `"stub"` — matches what's actually shipped today).
  Calls `validate_client_app_startup` before binding the local loopback
  listener; on failure, exits with a message naming exactly which env var
  to set (mirrors every other fail-fast in this codebase).

## Test plan
- Unit tests on `validate_client_app_startup`: production + stub + no
  override fails; production + stub + explicit `allow_stub_client=true`
  succeeds; production + `Real` kind succeeds; research + stub succeeds
  (today's default, unchanged); research + anything succeeds.
- Real smoke test of the binary (`cargo run -p conflux-node`, since
  `main.rs` isn't covered by `cargo test`): `CONFLUX_MODE=production`
  with no other overrides exits non-zero with a clear message; the same
  with `CONFLUX_CLIENT_APP_KIND=real` starts and binds its listener
  cleanly; the existing Phase 6 three-process smoke test path (research
  mode, stub client) is re-run unmodified to confirm zero behavior change
  there.

## Definition of done
- [x] `cargo test -p conflux-node` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` and `docs/FLOWER_COMPARISON.md` updated — gap 5
      marked closed, with the `conflux-server` → `conflux-node` location
      correction noted explicitly (not silently fixed).

## Outcome

`conflux-node::startup_guard` implemented exactly as specced —
`RuntimeMode`/`ClientAppKind` defined locally (no new `conflux-config`
dependency, preserving Phase 6's decision), `validate_client_app_startup`
a pure function with 5 unit tests covering every combination. `main.rs`
reads `CONFLUX_MODE`/`CONFLUX_ALLOW_STUB_CLIENT`/
`CONFLUX_CLIENT_APP_KIND` and calls the guard before ever attempting the
upstream connection to `conflux-server` — a production+stub deployment
now fails before any network I/O, not partway through.

Smoke-tested the binary directly: `CONFLUX_MODE=production` with no
overrides panics with `ProductionRefusesStubClient` before attempting to
connect upstream; `CONFLUX_CLIENT_APP_KIND=real` passes the guard cleanly
(the run then fails later at the upstream connect step, for the expected
reason — no server was actually listening — proving the guard itself
isn't what blocked it); the default research-mode path (no env vars set,
matching `docs/USAGE.md`'s quick-start) registers with a real running
`conflux-server` and binds its local listener exactly as before this
phase — zero regression for the existing Phase 6 three-process flow.

142 tests passing workspace-wide (was 131 combined with Phase 9a's 2 new
tests + this phase's 5), stable; `cargo fmt --check` and
`cargo clippy --workspace --all-targets` both clean.
