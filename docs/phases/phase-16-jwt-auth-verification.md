# Phase 16 — JWT auth verification

**Status: shipped 2026-08-30.**

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

- [x] `cargo test -p conflux-net -p conflux-server` passes, including the
      real signed/tampered-token end-to-end test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md`'s "Known deviations from spec" JWT bullet removed;
      `docs/FLOWER_COMPARISON.md` updated if it references this gap.

## Outcome

Shipped to this brief's shape. Four things it didn't specify, each of
which the implementation had to decide:

1. **The algorithm is pinned to the key, never read from the token.**
   A JWT carries its own `alg` header, and a verifier that trusts it can
   be handed a token claiming whatever algorithm suits the attacker —
   the classic algorithm-confusion attack. `JwtKeyMaterial` decides the
   algorithm once, when the key is loaded (RSA → RS256, ECDSA → ES256),
   and a token whose header disagrees is rejected before its signature
   is considered. This is also why the algorithm is *inferred* rather
   than configured: an RSA public key cannot verify an ES256 signature,
   so a separate setting could only be redundant or wrong.

2. **A new `DispatchError::Unauthenticated`, mapping to gRPC
   `Unauthenticated` (16).** The brief asked for the JWT rejection to
   stay "distinguishable, not conflated with the allow-list rejection
   path" — the existing `NotAllowed` maps to `PermissionDenied` (7),
   and gRPC already draws exactly the distinction needed: 16 means the
   credential was bad, 7 means the credential was fine and the caller
   still isn't authorized. Reusing `NotAllowed` would have sent an
   operator chasing an allow-list entry over an expired token.

3. **`exp` is required to be present, not merely validated.** Serde
   would happily accept a token with no `exp` claim and `Validation`
   would then have no expiry to reject. A JWT with no expiry is a bearer
   credential valid forever — precisely what an expiring token exists to
   avoid — so `exp` is in `required_spec_claims`. There is a test for
   the token that simply omits it.

4. **Two functions, not one.** The brief's five-case
   `verify_jwt_if_required` runs per-registration, but its
   production-no-key case was described as failing "fast at startup" —
   which a per-request function cannot do. `validate_jwt_startup` was
   added for that, called beside `resolve_server_tls` before the server
   binds. `verify_jwt_if_required` keeps the production check anyway:
   it is the function deciding whether an unverified caller gets in, and
   it should not depend on another function having run first to be safe.

Also worth recording: `ResolvedConfig` gained `topology` and `mode`
fields. The dispatcher needs the resolved mode to make this decision and
had no way to reach it — the axes were resolution *inputs* that never
appeared on the output. They're plain values, not `Resolved<T>`, because
they aren't layered: they're what *gives* every other field its
provenance.

`JwtKeyMaterial`'s `Debug` is hand-written and redacts the key. This one
holds only a public key, so a leak would be harmless — but deriving
`Debug` on key-material types is how the same shape ends up printing a
private one later.

18 new tests: 7 in `conflux-net::jwt` (valid, wrong key, expired,
tampered, `sub` mismatch, no-`exp`, unusable PEM), 4 on the enforcement
decision's five cases, and 7 real end-to-end
(`tests/jwt_auth_verification.rs`) over a live gRPC connection —
including both directions of gate independence: a valid token still
loses to an allow-list that excludes the client, and being allow-listed
under the presented token still doesn't excuse an expired one. 309 → 327
workspace-wide.

Verified against the real binary in all three smoke-test states:
production + `cross_device` (`auth = jwt` by default) with no key
refuses to start with `ProductionRequiresJwtKey`; with a real ES256 key
it logs `algorithm="ES256"` and proceeds past the gate; research with no
key warns that tokens will not be verified.
