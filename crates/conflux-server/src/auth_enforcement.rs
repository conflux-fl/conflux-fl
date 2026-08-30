//! Phase 9a — enforcing the resolved `auth` config value (spec §3 ties
//! topology to auth mode; `conflux-config` resolves and logs it, but
//! nothing previously read it to decide whether the gRPC server actually
//! binds with TLS). Closes gap 4 from `docs/FLOWER_COMPARISON.md`.
//!
//! Phase 16 added the `Jwt` arm's other half. `auth = "jwt"` no longer
//! just means "mTLS isn't required" — it now also means every
//! `register()` must present a token this deployment's public key can
//! verify. The two are independent: `resolve_server_tls` decides how the
//! socket is secured, `verify_jwt_if_required` decides whether a caller
//! proved who it is.

use conflux_config::{AuthMode, Mode};
use conflux_net::jwt::{JwtAuthError, JwtKeyMaterial, verify_token_for_client};
use conflux_net::tls::server_tls_config;
use tonic::transport::ServerTlsConfig;

/// Plain PEM bytes, matching every other Phase 7/8 backend's
/// "argument-based, not `conflux-config`-driven" precedent for connection
/// material (a cert path is a deployment detail, not an experiment-tuning
/// parameter).
pub struct TlsMaterial {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub client_ca_pem: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthEnforcementError {
    #[error(
        "mode = production with auth = mtls requires TLS material (set \
         CONFLUX_TLS_CERT_PATH, CONFLUX_TLS_KEY_PATH, and \
         CONFLUX_TLS_CLIENT_CA_PATH) — refusing to start a production \
         cross_silo deployment that would silently accept plaintext \
         connections"
    )]
    ProductionRequiresMtlsMaterial,
}

/// Decides whether the gRPC server should bind with TLS, and with what
/// config — a pure function (no I/O, no env reads) so every branch is
/// unit-testable without a real server, mirroring
/// `backend_selection::validate_production_backends`'s shape.
///
/// `Ok(None)` always means "bind plaintext" — either because `auth`
/// doesn't require TLS, or (research mode only) because it does but no
/// material was supplied and research's more permissive default applies.
pub fn resolve_server_tls(
    mode: Mode,
    auth: AuthMode,
    material: Option<TlsMaterial>,
) -> Result<Option<ServerTlsConfig>, AuthEnforcementError> {
    if auth != AuthMode::Mtls {
        return Ok(None);
    }

    match material {
        Some(material) => Ok(Some(server_tls_config(
            &material.cert_pem,
            &material.key_pem,
            &material.client_ca_pem,
        ))),
        None if mode == Mode::Production => {
            Err(AuthEnforcementError::ProductionRequiresMtlsMaterial)
        }
        // Research's deliberately more permissive default — falls back to
        // plaintext. The call site logs a warning; this function only
        // makes the decision, not the announcement (ADR 0007's "say so,
        // out loud" principle is a startup-log concern, not this pure
        // function's).
        None => Ok(None),
    }
}

/// Whether this deployment may start given its `auth` setting and
/// whatever JWT key material was supplied — the `Jwt` counterpart to
/// [`resolve_server_tls`]'s production check, and called at the same
/// point in startup.
///
/// Separate from [`verify_jwt_if_required`] below because the two answer
/// different questions at different times. This one runs once, before
/// the server binds, so a production deployment configured for JWT auth
/// with no key to verify against never starts — rather than starting
/// happily and rejecting (or worse, waving through) every registration
/// afterwards. A misconfiguration that only shows up on the first client
/// connection is a misconfiguration discovered by an outage.
pub fn validate_jwt_startup(
    mode: Mode,
    auth: AuthMode,
    key_material: Option<&JwtKeyMaterial>,
) -> Result<(), JwtAuthError> {
    if auth == AuthMode::Jwt && key_material.is_none() && mode == Mode::Production {
        return Err(JwtAuthError::ProductionRequiresJwtKey);
    }
    Ok(())
}

/// Verifies a presented `auth_token` when the resolved `auth` mode calls
/// for it — a pure function, no I/O, mirroring [`resolve_server_tls`]'s
/// shape so every branch is unit-testable without a live server.
///
/// Five cases, and the reasoning for each:
///
/// - `auth != Jwt` → `Ok(())`. JWT verification simply doesn't apply;
///   an mTLS deployment proves identity at the TLS layer instead.
/// - `auth == Jwt`, key present → verify signature, expiry, and that the
///   token's `sub` is the client actually registering.
/// - `auth == Jwt`, no key, `mode == Production` → refuse. Unreachable
///   in practice because [`validate_jwt_startup`] already refused to
///   start, and kept anyway: this function is the one that decides
///   whether an unverified caller gets in, so it should not depend on
///   another function having run first to be safe.
/// - `auth == Jwt`, no key, `mode == Research` → `Ok(())`. The same
///   permissive research default every other mode-owned relaxation in
///   this codebase uses; the call site logs it.
pub fn verify_jwt_if_required(
    mode: Mode,
    auth: AuthMode,
    key_material: Option<&JwtKeyMaterial>,
    presented_token: &str,
    client_id: &str,
) -> Result<(), JwtAuthError> {
    if auth != AuthMode::Jwt {
        return Ok(());
    }

    match key_material {
        Some(key) => verify_token_for_client(key, presented_token, client_id).map(|_| ()),
        None if mode == Mode::Production => Err(JwtAuthError::ProductionRequiresJwtKey),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material() -> TlsMaterial {
        // Not valid PEM — fine, since only `auth != Mtls` and the
        // "material absent" branches are unit-tested here. The
        // integration test exercises real material end-to-end.
        TlsMaterial {
            cert_pem: Vec::new(),
            key_pem: Vec::new(),
            client_ca_pem: Vec::new(),
        }
    }

    #[test]
    fn jwt_never_requires_tls_regardless_of_mode_or_material() {
        assert!(
            resolve_server_tls(Mode::Research, AuthMode::Jwt, None)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve_server_tls(Mode::Production, AuthMode::Jwt, None)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve_server_tls(Mode::Production, AuthMode::Jwt, Some(material()))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn mtls_with_material_binds_tls_in_either_mode() {
        assert!(
            resolve_server_tls(Mode::Research, AuthMode::Mtls, Some(material()))
                .unwrap()
                .is_some()
        );
        assert!(
            resolve_server_tls(Mode::Production, AuthMode::Mtls, Some(material()))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn mtls_with_no_material_in_research_falls_back_to_plaintext() {
        assert!(
            resolve_server_tls(Mode::Research, AuthMode::Mtls, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn mtls_with_no_material_in_production_fails_fast() {
        let err = resolve_server_tls(Mode::Production, AuthMode::Mtls, None).unwrap_err();
        assert!(matches!(
            err,
            AuthEnforcementError::ProductionRequiresMtlsMaterial
        ));
    }

    // --- Phase 16: the Jwt arm's five cases, one-for-one with the
    // resolve_server_tls cases above -----------------------------------

    /// A real ES256 keypair plus a signed token for `sub`, generated per
    /// call. Same approach as `conflux-net`'s own jwt tests: rcgen's
    /// default `KeyPair` is ECDSA P-256, so no private key is committed.
    fn keys_and_token(sub: &str) -> (JwtKeyMaterial, String) {
        use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
        use rcgen::KeyPair;

        #[derive(serde::Serialize)]
        struct TestClaims {
            sub: String,
            exp: u64,
        }

        let key_pair = KeyPair::generate().unwrap();
        let material = JwtKeyMaterial::from_public_key_pem(key_pair.public_key_pem().as_bytes())
            .expect("rcgen's default keypair is ECDSA P-256, i.e. ES256");
        let exp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let token = encode(
            &Header::new(Algorithm::ES256),
            &TestClaims {
                sub: sub.to_string(),
                exp,
            },
            &EncodingKey::from_ec_pem(key_pair.serialize_pem().as_bytes()).unwrap(),
        )
        .unwrap();
        (material, token)
    }

    #[test]
    fn mtls_mode_never_runs_jwt_verification() {
        // Not even a well-formed token — under `auth = mtls` the token is
        // simply not the thing proving identity, so nothing looks at it.
        for mode in [Mode::Research, Mode::Production] {
            verify_jwt_if_required(mode, AuthMode::Mtls, None, "not-a-jwt", "node-1").unwrap();
        }
    }

    #[test]
    fn jwt_with_a_key_accepts_a_valid_token_and_rejects_a_mismatched_subject() {
        let (material, token) = keys_and_token("node-1");

        for mode in [Mode::Research, Mode::Production] {
            verify_jwt_if_required(mode, AuthMode::Jwt, Some(&material), &token, "node-1").unwrap();

            // Genuine, unexpired, correctly signed — and still refused,
            // because it authenticates a different client.
            let err =
                verify_jwt_if_required(mode, AuthMode::Jwt, Some(&material), &token, "node-2")
                    .expect_err("node-1's token must not register node-2");
            assert!(
                matches!(err, JwtAuthError::SubjectMismatch { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn jwt_without_a_key_is_permitted_in_research_and_refused_in_production() {
        verify_jwt_if_required(Mode::Research, AuthMode::Jwt, None, "anything", "node-1")
            .expect("research's permissive default, same as the mTLS arm's");

        let err =
            verify_jwt_if_required(Mode::Production, AuthMode::Jwt, None, "anything", "node-1")
                .expect_err("production must not accept an unverified token");
        assert!(
            matches!(err, JwtAuthError::ProductionRequiresJwtKey),
            "{err:?}"
        );
    }

    #[test]
    fn startup_refuses_production_jwt_without_a_key_and_allows_every_other_combination() {
        let (material, _) = keys_and_token("node-1");

        let err = validate_jwt_startup(Mode::Production, AuthMode::Jwt, None)
            .expect_err("a production JWT deployment with no key cannot verify anything");
        assert!(
            matches!(err, JwtAuthError::ProductionRequiresJwtKey),
            "{err:?}"
        );

        validate_jwt_startup(Mode::Production, AuthMode::Jwt, Some(&material)).unwrap();
        validate_jwt_startup(Mode::Research, AuthMode::Jwt, None).unwrap();
        validate_jwt_startup(Mode::Production, AuthMode::Mtls, None).unwrap();
        validate_jwt_startup(Mode::Research, AuthMode::Mtls, None).unwrap();
    }
}
