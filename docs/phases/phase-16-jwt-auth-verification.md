# Phase 16 (draft) — JWT auth verification

**Status: scoping draft, not started.**

## Scope

Verify `RegisterRequest.auth_token` as a real, signed JWT when
`config.auth.value == AuthMode::Jwt` — the half of node authentication
`docs/phases/phase-9a-auth-enforcement.md` explicitly deferred
("`auth = jwt` continues to mean 'don't require mTLS,' which is accurate
today ... and not a regression"). Per spec §3's topology table, `jwt` is
the default `auth` mode for `cross_device`/`crowdsource`/`edge` — three
of Conflux's four topologies currently get no real cryptographic identity
check at `register()` beyond whatever `Phase 8c`'s allow-list-based
`SharedToken` comparison already provides (an opaque string match, not a
signature-verified, expiring, claims-bearing token).

**Relationship to the existing `SharedToken` path**: Phase 8b/8c already
built `NodeIdentity::SharedToken(String)` and wired it into `register()`'s
`node_allowlist.check()` call when `require_node_auth` is on — that path
treats `auth_token` as an opaque pre-shared secret, compared for
equality. This phase doesn't replace that path; it adds a *second*,
independent verification a token can be subject to: when `auth ==
AuthMode::Jwt`, `auth_token` is additionally required to be a valid,
correctly-signed, unexpired JWT — `require_node_auth`'s allow-list check
and this phase's JWT check are orthogonal (a deployment can run either,
both, or neither, matching every other mode-owned-parameter's
independent-toggle precedent in this codebase).

## Inputs

- `docs/phases/phase-9a-auth-enforcement.md`'s `resolve_server_tls`
  pattern — a pure, testable decision function taking `(mode, auth,
  material)` and returning a typed result, fed real material at the call
  site in `main.rs`. This phase's JWT verifier follows the identical
  shape: a pure function taking `(mode, auth, key_material, presented_token)`
  and returning `Result<Claims, JwtAuthError>`, tested exhaustively as a
  unit before ever touching `dispatcher.rs`.
- `docs/phases/phase-8c-node-auth-enforcement.md`'s `register()`
  enforcement call site (`dispatcher.rs`) — where this phase's check
  slots in, alongside (not instead of) the existing allow-list check.
- `conflux-config::AuthMode` (`Mtls | Jwt`) — already resolved and
  logged (ADR 0007); this phase is the first thing that actually reads
  it for the `Jwt` arm, mirroring Phase 9a's mTLS-arm precedent exactly.

## Deliverables

- New `conflux-net::jwt` module (co-located with `tls.rs`, same crate
  that already owns the mTLS mechanism — auth *mechanisms* live in
  `conflux-net`, auth *enforcement decisions* live in `conflux-server`,
  matching Phase 9a's existing split): `verify_token(key_material:
  &JwtKeyMaterial, token: &str) -> Result<Claims, JwtAuthError>` using a
  standard JWT crate (`jsonwebtoken`, the de facto standard choice —
  RS256/ES256 asymmetric signing, not HS256/shared-secret, since a
  shared-secret JWT would collapse to the same trust model
  `SharedToken` already provides with none of the added value).
  `JwtKeyMaterial` holds a PEM-encoded public key (verification only —
  Conflux's server never issues tokens itself; token issuance is an
  external identity-provider concern, out of scope, the same way
  Phase 7e's mTLS never became a CA).
- `Claims`: minimal, spec-consistent — `sub` (must equal the
  `RegisterRequest.client_id` being registered — a token valid for one
  client can't authenticate a different one), `exp` (standard expiry,
  rejected if past), `iat`. No custom claims beyond the JWT standard's
  own registered claim names — nothing Conflux-specific to keep the
  verifier interoperable with any standards-compliant IdP.
- `conflux-server::auth_enforcement` (Phase 9a's existing module): a new
  pure function `verify_jwt_if_required(mode: Mode, auth: AuthMode,
  key_material: Option<&JwtKeyMaterial>, presented_token: &str,
  client_id: &str) -> Result<(), JwtAuthError>` — mirrors
  `resolve_server_tls`'s five-case shape: `auth = Mtls` → always `Ok(())`
  (JWT verification doesn't apply); `auth = Jwt`, key material present →
  verify, reject on bad signature/expiry/`sub` mismatch; `auth = Jwt`, no
  key material, `mode = Production` → fails fast at startup (mirrors
  `ProductionRequiresMtlsMaterial`'s shape exactly, same "name what's
  missing" discipline); `auth = Jwt`, no key material, `mode = Research`
  → `Ok(())` with a logged warning (same permissive-research-default
  precedent as every other mode-owned relaxation).
- `main.rs`: reads `CONFLUX_JWT_PUBLIC_KEY_PATH` (PEM file, read at
  startup — same "argument-based, not config-driven" convention as
  `CONFLUX_TLS_CERT_PATH` in Phase 9a), builds `Option<JwtKeyMaterial>`.
- `dispatcher.rs`'s `register()`: calls `verify_jwt_if_required` before
  the existing `require_node_auth` allow-list check (fail on either,
  independently) — a request with a valid JWT but not on the allow-list
  still gets rejected if `require_node_auth` is on; a request with an
  invalid/expired JWT is rejected regardless of allow-list state.

## Test plan

- Unit tests on `verify_token`: valid signed token (real `jsonwebtoken`
  keypair, generated in-test) passes; wrong-key-signed token rejected;
  expired token rejected; `sub` mismatch against the presented
  `client_id` rejected — mirrors `resolve_server_tls`'s exhaustive-case
  unit-test discipline.
- Unit tests on `verify_jwt_if_required`'s five cases, matching Phase
  9a's own test-plan shape one-for-one (`Mtls` always `Ok`; `Jwt`+
  material in both modes; `Jwt`+no material research vs. production).
- Real end-to-end test (`tests/jwt_auth_verification.rs`, matching Phase
  9a's real-`rcgen`-material rigor): a live `register()` RPC call with a
  freshly-signed, valid token succeeds; the same call with a tampered
  token (one byte flipped in the signature) is rejected with a specific,
  distinguishable `tonic::Status` (not conflated with the allow-list
  rejection path) — the two failure reasons need to stay distinguishable
  for an operator debugging a rejected registration.
- Smoke test of the binary: production + `cross_device` (default
  `auth = jwt`) + no `CONFLUX_JWT_PUBLIC_KEY_PATH` set panics with a
  clear message; the same with a real key path starts cleanly — mirrors
  Phase 9a's own three-state smoke test exactly.

## Definition of done

- [ ] `cargo test -p conflux-net -p conflux-server` passes, including the
      real signed/tampered-token end-to-end test.
- [ ] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [ ] `docs/STATUS.md`'s "Known deviations from spec" JWT bullet removed;
      `docs/FLOWER_COMPARISON.md` updated if it references this gap.
