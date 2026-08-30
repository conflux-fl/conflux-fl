//! JWT verification for `RegisterRequest.auth_token`.
//!
//! Lives beside [`crate::tls`] for the same reason: auth *mechanisms*
//! belong to `conflux-net`, auth *enforcement decisions* belong to
//! `conflux-server`. This module knows how to check that a token is
//! genuine; it has no opinion about whether a given deployment should be
//! requiring one.
//!
//! **Verification only.** There is no signing key here and no way to
//! issue a token, deliberately. Conflux is not an identity provider —
//! tokens come from whatever IdP a deployment already runs, exactly as
//! certificates come from whatever CA it already runs (Phase 7e never
//! made this crate a CA either). That also means only the *public* half
//! of a keypair ever reaches a Conflux process.
//!
//! **Asymmetric only** — RS256 or ES256, never HS256. An HMAC-signed JWT
//! requires the verifier to hold the same secret the issuer signs with,
//! which is the identical trust model `NodeIdentity::SharedToken`
//! already provides (Phase 8b/8c) with none of the added value: any
//! party that can verify could also mint tokens for any client.

use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;

/// The registered JWT claims Conflux reads. Nothing Conflux-specific:
/// a token minted by any standards-compliant issuer works as-is, and
/// there is no proprietary claim for an IdP to be configured to emit.
///
/// `iat` is `Option` because RFC 7519 makes it optional; requiring it
/// would reject otherwise-valid tokens from issuers that omit it. `exp`
/// is not optional — a JWT with no expiry is a bearer credential valid
/// forever, which is the thing an expiring token exists to avoid.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    /// Subject — which client this token authenticates. Checked against
    /// the `client_id` actually being registered by
    /// [`verify_token_for_client`].
    pub sub: String,
    /// Expiry, seconds since the Unix epoch.
    pub exp: u64,
    /// Issued-at, seconds since the Unix epoch. Optional per RFC 7519.
    pub iat: Option<u64>,
}

/// A PEM-encoded public key, plus the one algorithm it may verify.
///
/// The algorithm is bound to the *key*, decided once when the key is
/// loaded, and is never read from the token being checked. That is the
/// whole point: a JWT carries its own `alg` header, and a verifier that
/// trusts that header can be handed a token claiming whatever algorithm
/// suits the attacker — the classic algorithm-confusion attack. Pinning
/// server-side means a token whose header disagrees is rejected before
/// its signature is even considered.
pub struct JwtKeyMaterial {
    decoding_key: DecodingKey,
    algorithm: Algorithm,
}

/// Written by hand rather than derived, so the key itself never reaches
/// a log line, a panic message, or an assertion failure. This one holds
/// only a public key, so the leak would be harmless — but the habit of
/// deriving `Debug` on key-material types is how the same code shape
/// ends up printing a private one later.
impl std::fmt::Debug for JwtKeyMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtKeyMaterial")
            .field("algorithm", &self.algorithm())
            .field("decoding_key", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JwtAuthError {
    #[error(
        "JWT public key is neither an RSA nor an ECDSA key in PEM form \
         (expected a PEM-encoded public key for RS256 or ES256): {message}"
    )]
    UnusableKey { message: String },
    #[error("JWT is expired")]
    Expired,
    #[error("JWT signature is not valid for the configured public key")]
    BadSignature,
    #[error(
        "JWT is signed with {presented}, but this deployment's key verifies \
         {expected} — refusing to verify a token that chose its own algorithm"
    )]
    WrongAlgorithm { expected: String, presented: String },
    #[error("JWT is malformed or missing a required claim: {message}")]
    Malformed { message: String },
    /// The token is genuine, but authenticates somebody else. Kept
    /// distinct from every failure above because it is the only one that
    /// is not a problem with the token itself — an operator seeing this
    /// has a valid credential wired to the wrong client id, which is a
    /// different fix from "your token expired."
    #[error("JWT is valid but issued for client {token_sub}, not {client_id}")]
    SubjectMismatch {
        token_sub: String,
        client_id: String,
    },
    #[error(
        "mode = production with auth = jwt requires a JWT public key (set \
         CONFLUX_JWT_PUBLIC_KEY_PATH) — refusing to start a production \
         deployment that would accept any auth_token without verifying it"
    )]
    ProductionRequiresJwtKey,
}

impl JwtKeyMaterial {
    /// Loads a PEM-encoded public key, inferring its algorithm from the
    /// key's own type — RSA keys verify RS256, ECDSA keys verify ES256.
    ///
    /// Inferred rather than configured because the key already
    /// determines it: an RSA public key cannot verify an ES256
    /// signature, so a separate `algorithm` setting could only ever be
    /// redundant or wrong. RSA is tried first; both loaders inspect the
    /// PEM's actual key type rather than just its label, so the order
    /// only decides which error surfaces for input that is neither.
    pub fn from_public_key_pem(pem: &[u8]) -> Result<Self, JwtAuthError> {
        if let Ok(decoding_key) = DecodingKey::from_rsa_pem(pem) {
            return Ok(Self {
                decoding_key,
                algorithm: Algorithm::RS256,
            });
        }
        match DecodingKey::from_ec_pem(pem) {
            Ok(decoding_key) => Ok(Self {
                decoding_key,
                algorithm: Algorithm::ES256,
            }),
            Err(e) => Err(JwtAuthError::UnusableKey {
                message: e.to_string(),
            }),
        }
    }

    pub fn algorithm(&self) -> &'static str {
        match self.algorithm {
            Algorithm::RS256 => "RS256",
            Algorithm::ES256 => "ES256",
            // Unreachable: `from_public_key_pem` only ever produces the
            // two above. Spelled out rather than `unreachable!` so a
            // future third algorithm is a compile-time prompt to name
            // it, not a runtime panic.
            other => match other {
                Algorithm::RS384 => "RS384",
                Algorithm::RS512 => "RS512",
                Algorithm::ES384 => "ES384",
                _ => "unsupported",
            },
        }
    }
}

/// Verifies a token's signature, expiry, and structure.
///
/// Does **not** check who the token is for — see
/// [`verify_token_for_client`], which is what a caller authenticating a
/// registration wants. This one is separate because "is this token
/// genuine" and "is this token yours" are different questions with
/// different fixes when they fail.
pub fn verify_token(key: &JwtKeyMaterial, token: &str) -> Result<Claims, JwtAuthError> {
    let mut validation = Validation::new(key.algorithm);
    // `exp` is validated by default; require it to be *present* too, so
    // a token that simply omits the claim can't slip past a check that
    // only rejects expiry it can see.
    validation.set_required_spec_claims(&["exp", "sub"]);
    // No audience is checked: `Claims` has no `aud`, and validating
    // against an audience Conflux never defines would reject every
    // real-world token for no gain.
    validation.validate_aud = false;

    match decode::<Claims>(token, &key.decoding_key, &validation) {
        Ok(data) => Ok(data.claims),
        Err(e) => Err(match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => JwtAuthError::Expired,
            jsonwebtoken::errors::ErrorKind::InvalidSignature => JwtAuthError::BadSignature,
            jsonwebtoken::errors::ErrorKind::InvalidAlgorithm => JwtAuthError::WrongAlgorithm {
                expected: key.algorithm().to_string(),
                // The token's own claimed algorithm, read only to
                // *report* the rejection — never to decide how to
                // verify.
                presented: jsonwebtoken::decode_header(token)
                    .map(|h| format!("{:?}", h.alg))
                    .unwrap_or_else(|_| "an unreadable algorithm".to_string()),
            },
            _ => JwtAuthError::Malformed {
                message: e.to_string(),
            },
        }),
    }
}

/// [`verify_token`], plus the check that the token was issued for the
/// client actually presenting it.
///
/// Without the `sub` comparison, any client holding any valid token
/// could register as any client id — the token would prove that
/// *somebody* was authenticated, which is not the question being asked
/// at registration.
pub fn verify_token_for_client(
    key: &JwtKeyMaterial,
    token: &str,
    client_id: &str,
) -> Result<Claims, JwtAuthError> {
    let claims = verify_token(key, token)?;
    if claims.sub != client_id {
        return Err(JwtAuthError::SubjectMismatch {
            token_sub: claims.sub,
            client_id: client_id.to_string(),
        });
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use rcgen::KeyPair;
    use serde::Serialize;

    /// A real ES256 keypair, generated per test rather than checked in.
    /// `rcgen::KeyPair::generate()` is ECDSA P-256 — exactly ES256's
    /// curve — and `conflux-net` already depends on rcgen for the mTLS
    /// tests, so this needs no new dependency and no private key
    /// committed to the repository.
    struct TestKeys {
        public_pem: String,
        encoding: EncodingKey,
    }

    fn test_keys() -> TestKeys {
        let key_pair = KeyPair::generate().unwrap();
        TestKeys {
            public_pem: key_pair.public_key_pem(),
            encoding: EncodingKey::from_ec_pem(key_pair.serialize_pem().as_bytes()).unwrap(),
        }
    }

    #[derive(Serialize)]
    struct TestClaims {
        sub: String,
        exp: u64,
        iat: u64,
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn sign(keys: &TestKeys, sub: &str, exp: u64) -> String {
        encode(
            &Header::new(Algorithm::ES256),
            &TestClaims {
                sub: sub.to_string(),
                exp,
                iat: now(),
            },
            &keys.encoding,
        )
        .unwrap()
    }

    #[test]
    fn a_validly_signed_unexpired_token_verifies() {
        let keys = test_keys();
        let material = JwtKeyMaterial::from_public_key_pem(keys.public_pem.as_bytes()).unwrap();
        assert_eq!(material.algorithm(), "ES256");

        let claims =
            verify_token_for_client(&material, &sign(&keys, "node-1", now() + 3600), "node-1")
                .unwrap();

        assert_eq!(claims.sub, "node-1");
        assert!(claims.iat.is_some());
    }

    #[test]
    fn a_token_signed_by_a_different_key_is_rejected() {
        let issuer = test_keys();
        let impostor = test_keys();
        let material = JwtKeyMaterial::from_public_key_pem(issuer.public_pem.as_bytes()).unwrap();

        let err = verify_token(&material, &sign(&impostor, "node-1", now() + 3600))
            .expect_err("a token from an unknown issuer must not verify");

        assert!(matches!(err, JwtAuthError::BadSignature), "{err:?}");
    }

    #[test]
    fn an_expired_token_is_rejected_and_says_so_specifically() {
        let keys = test_keys();
        let material = JwtKeyMaterial::from_public_key_pem(keys.public_pem.as_bytes()).unwrap();

        // Well past `Validation`'s default 60s leeway.
        let err = verify_token(&material, &sign(&keys, "node-1", now() - 7200))
            .expect_err("an expired token must not verify");

        assert!(matches!(err, JwtAuthError::Expired), "{err:?}");
    }

    #[test]
    fn a_valid_token_for_another_client_cannot_register_this_one() {
        let keys = test_keys();
        let material = JwtKeyMaterial::from_public_key_pem(keys.public_pem.as_bytes()).unwrap();
        let token = sign(&keys, "node-1", now() + 3600);

        // The signature is genuine and unexpired — only the subject is
        // wrong, which is exactly the case a signature check alone
        // would wave through.
        verify_token(&material, &token).expect("the token itself is valid");
        let err = verify_token_for_client(&material, &token, "node-2")
            .expect_err("node-1's token must not authenticate node-2");

        match err {
            JwtAuthError::SubjectMismatch {
                token_sub,
                client_id,
            } => {
                assert_eq!(token_sub, "node-1");
                assert_eq!(client_id, "node-2");
            }
            other => panic!("expected a subject mismatch, got {other:?}"),
        }
    }

    #[test]
    fn a_tampered_signature_is_rejected() {
        let keys = test_keys();
        let material = JwtKeyMaterial::from_public_key_pem(keys.public_pem.as_bytes()).unwrap();
        let token = sign(&keys, "node-1", now() + 3600);

        // Flip one character of the signature segment.
        let (body, signature) = token.rsplit_once('.').unwrap();
        let mut bytes: Vec<char> = signature.chars().collect();
        bytes[0] = if bytes[0] == 'A' { 'B' } else { 'A' };
        let tampered = format!("{body}.{}", bytes.into_iter().collect::<String>());

        let err = verify_token(&material, &tampered).expect_err("a tampered token must not verify");
        assert!(
            matches!(
                err,
                JwtAuthError::BadSignature | JwtAuthError::Malformed { .. }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn a_token_with_no_expiry_is_rejected_rather_than_treated_as_never_expiring() {
        #[derive(Serialize)]
        struct NoExp {
            sub: String,
        }
        let keys = test_keys();
        let material = JwtKeyMaterial::from_public_key_pem(keys.public_pem.as_bytes()).unwrap();
        let token = encode(
            &Header::new(Algorithm::ES256),
            &NoExp {
                sub: "node-1".to_string(),
            },
            &keys.encoding,
        )
        .unwrap();

        let err =
            verify_token(&material, &token).expect_err("a token without exp must not be accepted");
        assert!(matches!(err, JwtAuthError::Malformed { .. }), "{err:?}");
    }

    #[test]
    fn a_pem_that_is_not_a_usable_public_key_is_refused_at_load_time() {
        let err = JwtKeyMaterial::from_public_key_pem(
            b"-----BEGIN PUBLIC KEY-----\nnope\n-----END PUBLIC KEY-----",
        )
        .expect_err("garbage is not a key");
        assert!(matches!(err, JwtAuthError::UnusableKey { .. }), "{err:?}");
    }
}
