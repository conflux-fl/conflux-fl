# Phase 9a — Enforcing the resolved `auth` config value

## Scope
Closes gap 4 from `docs/FLOWER_COMPARISON.md`: `conflux-config` resolves
and logs `auth` (`mtls` for `cross_silo`, `jwt` everywhere else, spec §3)
correctly, but nothing reads `config.auth.value` at startup and decides
whether the gRPC server actually binds with TLS. The mTLS *mechanism*
(`conflux-net::tls`, Phase 7e) exists and is proven with real handshake
tests; this phase makes the already-resolved decision real.

JWT auth itself (verifying a `RegisterRequest.auth_token` as a real JWT)
is **not** in scope — it's a separate, larger, already-tracked deviation
(`docs/STATUS.md`'s "Known deviations from spec"). This phase only makes
the *mTLS* half of `auth` actually enforced; `auth = "jwt"` continues to
mean "don't require mTLS," which is accurate today (JWT verification
isn't implemented yet) and not a regression.

## Inputs
- `conflux-config::AuthMode` (`Mtls | Jwt`) and `ResolvedConfig.auth` —
  already resolved and logged (ADR 0007), just never read past that.
- `conflux-net::tls::server_tls_config` (Phase 7e) — the exact builder
  this phase wires in, unchanged.
- `conflux-server`'s `backend_selection.rs`/`validate_production_backends`
  (Phase 8a) and `require_node_auth` (Phase 8b) — the precedent for a
  mode-driven fail-fast that names exactly what's missing, which this
  phase's production behavior follows.

## Deliverables
- `conflux-server::auth_enforcement` (new module): a pure, testable
  decision function —
  `resolve_server_tls(mode: Mode, auth: AuthMode, material: Option<TlsMaterial>) -> Result<Option<ServerTlsConfig>, AuthEnforcementError>`
  — where `TlsMaterial { cert_pem, key_pem, client_ca_pem }` is plain PEM
  bytes (matching every other Phase 7/8 backend's "argument-based, not
  config-driven" precedent for connection material).
  - `auth = Jwt`: always `Ok(None)` — no TLS required, regardless of
    `material`.
  - `auth = Mtls`, `material` present: `Ok(Some(server_tls_config(...)))`.
  - `auth = Mtls`, `material` absent, `mode = Production`: fails fast —
    `Err(AuthEnforcementError::ProductionRequiresMtlsMaterial)`, naming
    the missing env vars, mirroring `validate_production_backends`'s
    shape.
  - `auth = Mtls`, `material` absent, `mode = Research`: `Ok(None)` — a
    deliberately more permissive research default (falls back to
    plaintext with a logged warning at the call site), consistent with
    every other mode-owned relaxation in this codebase.
- `main.rs`: reads `CONFLUX_TLS_CERT_PATH`/`CONFLUX_TLS_KEY_PATH`/
  `CONFLUX_TLS_CLIENT_CA_PATH` (file paths, read at startup), builds
  `Option<TlsMaterial>`, calls `resolve_server_tls`, and conditionally
  calls `.tls_config(...)` on the gRPC `Server::builder()` — the HTTP
  admin server is unaffected (spec never ties `auth` to it).

## Test plan
- Unit tests on `resolve_server_tls` covering all five cases above
  (Jwt always `None`; Mtls+material present in both modes; Mtls+no
  material in research vs. production).
- Real end-to-end test: `resolve_server_tls` fed real `rcgen`-generated
  cert material, its `Some(ServerTlsConfig)` actually bound to a live
  tonic server — a trusted-CA client connects, a plaintext client is
  rejected (same rigor as Phase 7e's `mtls.rs`, proving the config-driven
  path produces a working server, not just that it type-checks).
- Real smoke test of the binary itself (`cargo run`, since `main.rs`
  isn't covered by `cargo test`): `CONFLUX_TOPOLOGY=cross_silo
  CONFLUX_MODE=production` with no TLS env vars set panics with a clear
  message; the same with real cert paths set starts cleanly.

## Definition of done
- [x] `cargo test -p conflux-server` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` and `docs/FLOWER_COMPARISON.md` updated — gap 4
      marked closed.

## Outcome

`auth_enforcement.rs`'s `resolve_server_tls` implemented exactly as
specced — a pure function, unit-tested for all five cases. `main.rs`
reads `CONFLUX_TLS_CERT_PATH`/`CONFLUX_TLS_KEY_PATH`/
`CONFLUX_TLS_CLIENT_CA_PATH`, calls it, and conditionally applies
`.tls_config(...)` to the gRPC server builder; a research-mode fallback
to plaintext logs a `tracing::warn!`.

Real tests (`tests/auth_enforcement.rs`): real `rcgen` material fed
through `resolve_server_tls` produces a `ServerTlsConfig` that a live
tonic server binds with — a trusted-CA client completes a real RPC, a
plaintext client is rejected; separately, `auth = jwt` produces `None`
and a plaintext server that a plaintext client can use normally (proving
zero regression for every non-`cross_silo` deployment).

Smoke-tested the binary directly in all three states: research +
`cross_silo` + no TLS material → plaintext fallback with the warning
logged; production + `cross_silo` + no material → panics with
`ProductionRequiresMtlsMaterial`; production + `cross_silo` + real
material + real durable backends (Redis/Postgres) → starts clean, no
panics, no warnings.

`cargo test -p conflux-server` fully green, `cargo fmt --check` and
`cargo clippy --workspace --all-targets` both clean.
