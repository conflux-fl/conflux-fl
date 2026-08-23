//! Extracts a peer certificate's fingerprint from an authenticated
//! `tonic::Request`, for Phase 8b/8c's node-auth allow-list
//! (`conflux_registry::NodeIdentity::CertFingerprint`).
//!
//! See `docs/phases/phase-8c-node-auth-enforcement.md`.

use sha2::{Digest, Sha256};
use tonic::Request;
use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo};

/// SHA-256 hex digest of the peer's leaf certificate (DER-encoded), when
/// the connection used mTLS and the server verified a client cert.
///
/// `None` is a normal, expected case, not an error: no TLS at all (the
/// node↔`ClientApp` loopback hop, ADR 0004, or a deployment that uses
/// `NodeIdentity::SharedToken` instead), or TLS without
/// `client_ca_root` configured. Callers fall back to the request's
/// `auth_token` field in either case.
pub fn peer_cert_fingerprint<T>(request: &Request<T>) -> Option<String> {
    let tls_info = request
        .extensions()
        .get::<TlsConnectInfo<TcpConnectInfo>>()?;
    let certs = tls_info.peer_certs()?;
    let leaf = certs.first()?;

    let mut hasher = Sha256::new();
    hasher.update(leaf.as_ref());
    let digest = hasher.finalize();

    Some(digest.iter().map(|b| format!("{b:02x}")).collect())
}
