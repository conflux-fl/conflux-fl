//! Phase 9a — enforcing the resolved `auth` config value (spec §3 ties
//! topology to auth mode; `conflux-config` resolves and logs it, but
//! nothing previously read it to decide whether the gRPC server actually
//! binds with TLS). Closes gap 4 from `docs/FLOWER_COMPARISON.md`.
//!
//! JWT verification itself stays out of scope (see the phase brief) —
//! `auth = "jwt"` continues to just mean "mTLS isn't required."

use conflux_config::{AuthMode, Mode};
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
}
