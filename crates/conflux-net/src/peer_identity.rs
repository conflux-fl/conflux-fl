//! Extracts a peer certificate's fingerprint from an authenticated
//! `tonic::Request`, for node-auth allow-list checks
//! (`conflux_registry::NodeIdentity::CertFingerprint`): when node auth is
//! enforced, a registering client's presented identity is this fingerprint
//! if the connection used mTLS, or its shared token otherwise, and either
//! form is checked against the same allow-list.

use sha2::{Digest, Sha256};
use tonic::Request;
use tonic::transport::server::{TcpConnectInfo, TlsConnectInfo};

/// SHA-256 hex digest of the peer's leaf certificate (DER-encoded), when
/// the connection used mTLS and the server verified a client cert.
///
/// `None` is a normal, expected case, not an error: no TLS at all (the
/// local loopback hop between `conflux-node` and its `ClientApp`
/// never uses TLS, or a deployment may simply use
/// `NodeIdentity::SharedToken` instead), or TLS without `client_ca_root`
/// configured so no client cert was requested or verified. Callers fall
/// back to the request's `auth_token` field in either case.
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
