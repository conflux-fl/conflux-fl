# Phase 7e — mTLS for push mode

## Scope
Real mutual TLS for `conflux-net`'s gRPC transport — spec §3 ties
`cross_silo`'s push mode to mTLS auth (few, trusted, always-reachable
institutions; certificate-based mutual auth fits that trust model better
than JWT). Adds TLS-aware connect/serve helpers to `conflux-net`, proven
with a real handshake: a client presenting a cert signed by the configured
CA connects and completes an RPC; a client presenting a cert from an
*untrusted* CA is rejected at the TLS layer, before any RPC logic runs.

**Does not build**: real certificate provisioning/rotation (out of scope —
this phase takes PEM bytes from wherever the caller gets them, the same
"argument-based, not `conflux-config`-driven" precedent as every other
Phase 7 backend), or JWT auth for pull mode (spec §3 ties pull mode to
JWT, but that's a different auth mechanism entirely and its own scope).

## Inputs
- Spec §3's table: `cross_silo` is push + mTLS; every other topology is
  pull + JWT.
- `conflux-net::{PullTransport, PushTransport, FlTransportService}`
  (Phase 3) — the connect/serve surface this phase adds TLS variants to,
  not replaces (plaintext stays available for research-mode/local use).
- `tonic`'s `transport` feature already provides `ServerTlsConfig`/
  `ClientTlsConfig`/`Identity`/`Certificate`; needs the `tls-ring` feature
  enabled to actually perform TLS (not just construct the config types).

## Deliverables
- `conflux-net::tls`: `server_tls_config(cert_pem, key_pem, client_ca_pem)
  -> ServerTlsConfig` (sets `client_ca_root`, which is what makes this
  *mutual* TLS — the server requires and verifies a client cert, not just
  serves its own) and `client_tls_config(cert_pem, key_pem, server_ca_pem,
  domain) -> ClientTlsConfig`.
- `PullTransport::connect_with_tls`/`PushTransport::connect_with_tls` —
  TLS-aware siblings of the existing plaintext `connect`.
- Test-only cert generation via `rcgen` (dev-dependency): a CA, a server
  cert signed by it, a "good" client cert signed by the same CA, and a
  "bad" client cert signed by an entirely different, untrusted CA — the
  actual proof that mTLS *rejects* wrong-CA clients, not just that TLS is
  technically on.

## Test plan
- Real handshake tests (no mocking rustls/tonic's TLS internals): bind
  `FlTransportService` with mTLS required, connect with the good client
  cert, complete a real RPC (e.g. `Register`) successfully.
- Connect with the bad-CA client cert against the same mTLS-required
  server: the connection attempt itself fails (TLS handshake rejection),
  before any RPC is even attempted — this is the test that actually proves
  mutual auth is enforced, not merely configured.
- A plaintext `connect` (no TLS) against an mTLS-required server also
  fails, confirming the server doesn't silently accept downgraded
  connections.

## Definition of done
- [x] `cargo test -p conflux-net` passes, including the bad-CA rejection
      test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.

## Implementation note
The bad-CA rejection test's first draft asserted `connect_with_tls(...)`
itself returns `Err`. It didn't — tonic's `Endpoint::connect()` can
succeed before the TLS handshake has actually completed (it completes
lazily, on first real use), so the untrusted-CA rejection only surfaced on
the first RPC attempt, not at `connect()`. Fixed by checking either
outcome (connect failing, or the first RPC failing) — both prove the same
thing: this client never gets a successful RPC through. Applied the same
"either outcome" shape to the plaintext-rejection test too.
