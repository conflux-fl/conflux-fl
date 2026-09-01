//! Authentication for the HTTP admin surface.
//!
//! The gRPC surface has had real authentication (node
//! allow-list) and (JWT). The HTTP surface, serving the same
//! process on a different port, had none at all — and it is the more
//! dangerous of the two, because `/admin/allowlist` is the endpoint that
//! decides *who is allowed to participate*. Anything able to reach that
//! port could add itself to the allow-list and then present a
//! legitimately-accepted identity on the gRPC side. The strong
//! authentication on one port was gated by an unauthenticated write on
//! the other.
//!
//! `docs/WEB_APP_INTEGRATION.md` already described the mitigation —
//! never expose the admin port, treat your own backend as the trust
//! boundary — and that remains good advice. It is not a control the
//! framework enforces, though, and "acceptable as long as it stays
//! unreachable" stopped being sufficient once `CONFLUX_HTTP_ADDR` made
//! binding beyond loopback a single environment variable.
//!
//! ## The policy
//!
//! - `/health` is always open. Liveness probes come from load balancers
//!   and orchestrators that have no way to hold a secret, and it reveals
//!   nothing beyond "a process is running".
//! - Every other route requires `Authorization: Bearer <token>` when a
//!   token is configured.
//! - With no token configured, every route is open — exactly the
//!   previous behavior, so a loopback-only development deployment is
//!   unaffected.
//! - Binding beyond loopback with no token configured **refuses to
//!   start**. That combination is the one that turns a documented
//!   caveat into an exposed control plane, and it is always a mistake.

use std::net::SocketAddr;

use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// The configured admin bearer token, or `None` when the admin surface
/// is unauthenticated.
///
/// Deliberately not `Debug`/`Display`: this is a bearer credential, and
/// the way it ends up in a log is a type that derives its way there.
#[derive(Clone)]
pub struct AdminToken(String);

impl AdminToken {
    /// Wraps a configured token value.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// Compares in constant time with respect to the *content* of the
    /// candidate.
    ///
    /// A naive `==` on strings returns as soon as two bytes differ, so an
    /// attacker who can time many requests can recover the token one byte
    /// at a time. The length check below is not constant-time and does not
    /// need to be — the length of a token is not the secret, and treating
    /// it as one would mean hashing on every request to hide something an
    /// attacker gains nothing from.
    fn matches(&self, candidate: &str) -> bool {
        let expected = self.0.as_bytes();
        let got = candidate.as_bytes();
        if expected.len() != got.len() {
            return false;
        }
        let mut difference = 0u8;
        for (a, b) in expected.iter().zip(got) {
            difference |= a ^ b;
        }
        difference == 0
    }
}

#[derive(Debug, thiserror::Error)]
/// Why the admin API's configuration is refused at startup.
pub enum AdminAuthError {
    #[error(
        "the HTTP admin API is bound to {addr}, which is reachable beyond loopback, but no \
         CONFLUX_ADMIN_TOKEN is set — refusing to start. /admin/allowlist decides who may \
         participate in this experiment; exposing it unauthenticated lets anyone who can \
         reach the port add themselves. Either set CONFLUX_ADMIN_TOKEN, or bind to \
         127.0.0.1 and reach it through your own authenticated backend."
    )]
    /// The admin API is bound somewhere reachable with no token set.
    ExposedWithoutToken {
        /// The address the admin API was asked to bind.
        addr: String,
    },
}

/// Whether the server may start, given where the admin API is bound and
/// whether a token was configured.
///
/// A pure function, mirroring `auth_enforcement`'s `resolve_server_tls`
/// and `validate_jwt_startup` — same shape, same place in startup, so
/// every "is this deployment safe to run" decision is testable without a
/// live server.
pub fn validate_admin_binding(
    addr: SocketAddr,
    token: Option<&AdminToken>,
) -> Result<(), AdminAuthError> {
    if token.is_some() || addr.ip().is_loopback() {
        return Ok(());
    }
    Err(AdminAuthError::ExposedWithoutToken {
        addr: addr.to_string(),
    })
}

/// Extracts a bearer token from an `Authorization` header.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

/// Axum middleware enforcing [the policy](self) on every request.
pub async fn require_admin_token(
    token: Option<AdminToken>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Always open, and checked before anything else so a probe never
    // depends on credential handling working.
    if request.uri().path() == "/health" {
        return Ok(next.run(request).await);
    }

    let Some(token) = token else {
        // Unauthenticated mode. Reaching here means the server started,
        // which means `validate_admin_binding` already established the
        // listener is loopback-only.
        return Ok(next.run(request).await);
    };

    match bearer(request.headers()) {
        Some(presented) if token.matches(presented) => Ok(next.run(request).await),
        // No body, and the same status for "absent" as for "wrong" — an
        // error that distinguishes them tells an attacker which half of
        // the guess was right.
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn loopback_without_a_token_may_start() {
        // The existing development default, preserved.
        validate_admin_binding(addr("127.0.0.1:8080"), None).unwrap();
        validate_admin_binding(addr("[::1]:8080"), None).unwrap();
    }

    #[test]
    fn binding_beyond_loopback_without_a_token_refuses_to_start() {
        for a in ["0.0.0.0:8080", "192.168.1.10:8080", "[::]:8080"] {
            let err = validate_admin_binding(addr(a), None)
                .expect_err("an exposed unauthenticated admin API must refuse to start");
            assert!(matches!(err, AdminAuthError::ExposedWithoutToken { .. }));
            // The message must name the address; an operator seeing this
            // needs to know which binding triggered it.
            assert!(err.to_string().contains(a), "{err}");
        }
    }

    #[test]
    fn a_token_permits_any_binding() {
        let token = AdminToken::new("s3cret");
        for a in ["127.0.0.1:8080", "0.0.0.0:8080", "192.168.1.10:8080"] {
            validate_admin_binding(addr(a), Some(&token)).unwrap();
        }
    }

    #[test]
    fn token_comparison_accepts_only_the_exact_value() {
        let token = AdminToken::new("correct-horse-battery-staple");
        assert!(token.matches("correct-horse-battery-staple"));
        assert!(!token.matches("correct-horse-battery-stapl"));
        assert!(!token.matches("correct-horse-battery-staplee"));
        assert!(!token.matches("Correct-horse-battery-staple"));
        assert!(!token.matches(""));
    }

    #[test]
    fn bearer_parsing_requires_the_scheme() {
        let mut headers = HeaderMap::new();
        assert_eq!(bearer(&headers), None);

        headers.insert(axum::http::header::AUTHORIZATION, "abc".parse().unwrap());
        assert_eq!(bearer(&headers), None, "a bare value is not a bearer token");

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Basic abc".parse().unwrap(),
        );
        assert_eq!(bearer(&headers), None, "Basic is not Bearer");

        headers.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer abc".parse().unwrap(),
        );
        assert_eq!(bearer(&headers), Some("abc"));
    }
}
