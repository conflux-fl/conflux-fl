# Phase 8c — Node authentication: enforcement + admin surface

## Scope
Wire Phase 8b's allow-list into the actual `Register` RPC, and give an
operator a way to populate/revoke it — closing gap 2 from
`docs/FLOWER_COMPARISON.md` for real, not just building the data model.
This is also where gap 3 (mTLS proves CA trust, not per-node identity or
revocability) gets closed: once a specific client's certificate identity
is checked against an explicit allow-list at `register()` time, revoking
one node not only becomes possible — mirroring `flwr supernode
unregister` — it no longer requires reissuing the CA the way pulling
access under mTLS-alone would have.

## Inputs
- `conflux-net`'s mTLS (Phase 7e) — the source of `NodeIdentity::
  CertFingerprint` when a connection is authenticated with a client
  certificate. Requires tonic's `tls-connect-info` feature to actually
  extract the peer certificate from an established connection.
- `conflux-server::AppState`/`RoundDispatcher` (Phase 5) — `register()`'s
  existing implementation (`dispatcher.rs`) is the one enforcement point;
  every other RPC is unaffected.
- `conflux-server`'s HTTP admin surface (Phase 5: `/health`,
  `/round/status`, `/clients/register`) — the natural home for the new
  allow-list management endpoints, same pattern as the existing
  `/clients/register` (an admin/observability entry point, not a second
  source of truth).

## Deliverables
- `conflux-net`: a helper extracting a peer certificate's SHA-256
  fingerprint from an authenticated `tonic::Request<T>` (via
  `tonic::transport::server::TlsConnectInfo`) — `None` when the connection
  isn't using mTLS, which is a normal, expected case, not an error.
- `conflux-server::AppState`: gains `node_allowlist: Arc<AnyNodeAllowlist>`
  — always constructed (even when `require_node_auth = false`), so
  toggling the parameter doesn't need any other wiring change, only a
  config value change and a restart (config resolution is startup-only
  everywhere else in this codebase; this doesn't invent a new exception).
- `dispatcher.rs`'s `register()`: when `config.require_node_auth.value`,
  determine the presented `NodeIdentity` (the connection's peer cert
  fingerprint if present, else `SharedToken(request.auth_token)`), call
  `node_allowlist.check(client_id, presented)`, and reject with a
  `DispatchError` that maps to `tonic::Status::permission_denied` if it
  doesn't match — *before* touching `conflux-registry` at all, so a
  rejected node never even shows up as a lifecycle registration attempt.
  When `require_node_auth` is `false`, `register()` behaves exactly as it
  does today — zero behavior change for research/default deployments.
- HTTP admin endpoints, mirroring `flwr supernode register`/`list`/
  `unregister`: `POST /admin/allowlist` (body: `client_id` +
  `identity` — either a cert fingerprint or a shared token),
  `DELETE /admin/allowlist/{client_id}`, `GET /admin/allowlist`.
- `main.rs`: the allow-list backend follows the same choice as the
  registry backend (`CONFLUX_REGISTRY_BACKEND=redis` ⇒
  `RedisNodeAllowlist` too) — a deliberate simplification (one fewer env
  var) rather than a fully independent fourth backend axis; documented as
  such, not silently assumed.

## Test plan
- Real end-to-end tests, same rigor as Phase 7e's mTLS tests: with
  `require_node_auth = true`, a client whose cert fingerprint (or shared
  token) was `allow`-ed registers successfully; a client with a *valid
  mTLS cert signed by the trusted CA* but whose identity was never added
  to the allow-list is rejected — this is the specific new rejection case
  the Flower comparison flagged as missing (CA trust alone isn't enough);
  a `SharedToken`-based client with the wrong token is rejected; after
  `revoke`, a previously-allowed client's next registration attempt fails.
- With `require_node_auth = false` (the research default), every existing
  Phase 5/6 registration test keeps passing completely unmodified — proof
  the toggle actually has zero effect when off, not just that it compiles
  when off.
- HTTP admin endpoints exercised as real request/response round trips
  (same `tower::ServiceExt::oneshot` pattern as the existing admin tests):
  add via `POST`, confirm via `GET`, revoke via `DELETE`, confirm removal.

## Definition of done
- [x] `cargo test -p conflux-net -p conflux-server` passes, including the
      cert-valid-but-not-allow-listed rejection test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] Every pre-existing test (Phases 0–7) still passes unmodified with
      `require_node_auth` defaulting off.
- [x] `docs/STATUS.md` updated — this should let `FLOWER_COMPARISON.md`'s
      gaps 2 and 3 be marked closed, not just tracked.

## Outcome

`conflux-net` gained `peer_cert_fingerprint(&Request<T>) -> Option<String>`
(`peer_identity.rs`), reading `TlsConnectInfo<TcpConnectInfo>` from the
request's extensions (requires tonic's `tls-connect-info` feature, added
to `conflux-net`'s and `conflux-server`'s `Cargo.toml`) and SHA-256-hashing
the leaf cert's DER bytes. `RoundDispatcher::register` grew a fourth
parameter, `peer_cert_fingerprint: Option<&str>` — every implementation
across the workspace (`conflux-server`'s real one, `conflux-node`'s
loopback one, and both crates' test doubles) updated accordingly.
`service.rs`'s `register()` extracts the fingerprint *before*
`request.into_inner()`, since it lives in the request's extensions, not
its body.

`AppState` gained `node_allowlist: Arc<AnyNodeAllowlist>`, always
constructed — `AppState::new`/`new_with_persistent_accounting[_table]` use
`InMemoryNodeAllowlist`; `AppState::connect` derives the allow-list
backend from `backends.registry` (Redis registry ⇒ `RedisNodeAllowlist`),
per the brief's stated simplification. `conflux-server::dispatcher.rs`'s
`register()` now checks `config.require_node_auth.value` first: if set,
it builds the presented `NodeIdentity` (peer cert fingerprint if present,
else `SharedToken(auth_token)`), calls `node_allowlist.check`, and returns
`DispatchError::NotAllowed` (→ `Status::permission_denied`) before ever
touching `conflux-registry` — a rejected node never appears as a
lifecycle registration attempt. When the flag is off, the code path is
identical to pre-Phase-8.

HTTP admin surface: `POST /admin/allowlist` (body: `client_id` +
`identity` tagged `cert_fingerprint`/`shared_token`), `DELETE
/admin/allowlist/{client_id}`, `GET /admin/allowlist`.

Real end-to-end tests (`crates/conflux-server/tests/node_auth.rs`, 7
tests): a `SharedToken`-allowed client registers; a wrong token is
rejected; a never-allowed client is rejected; a revoked client is
rejected; `require_node_auth = false` keeps registration working with an
empty allow-list (the "off means off" proof); and — the specific case the
Flower comparison flagged — an mTLS client whose fingerprint *is*
allow-listed registers, while one whose cert is signed by the same
trusted CA but was never `allow`-ed is rejected even though the TLS
handshake itself succeeds. Plus one real fingerprint-extraction test in
`conflux-net/tests/mtls.rs` (a live mTLS handshake, comparing
`peer_cert_fingerprint`'s output against an independently computed SHA-256
of the client cert's DER bytes) and one HTTP admin round-trip test
(`admin_allowlist_add_list_revoke_round_trip`).

131 tests passing workspace-wide (was 122 at the end of Phase 8b), stable
across repeated runs. `cargo fmt --check` and
`cargo clippy --workspace --all-targets` both clean. Smoke-tested the
binary directly against real Redis + Postgres with
`CONFLUX_MODE=production`: `require_node_auth` resolves `true` and is
logged, `RedisNodeAllowlist` connects successfully, no panics.

This closes gap 2 (no node-identity check at all) and gap 3 (mTLS proved
CA trust, not per-node identity or revocability) from
`docs/FLOWER_COMPARISON.md` — see that document's own update.
