//! Mutual TLS config for the gRPC transport — spec §3 ties `cross_silo`'s
//! push mode to mTLS. See `docs/phases/phase-7e-mtls.md`.
//!
//! Takes PEM bytes from wherever the caller sourced them; real
//! certificate provisioning/rotation is out of scope for this phase, same
//! as every other Phase 7 backend's "argument-based, not
//! `conflux-config`-driven" precedent.

use tonic::transport::{Certificate, ClientTlsConfig, Identity, ServerTlsConfig};

/// `client_ca_root` is what makes this *mutual* TLS rather than plain
/// server-side TLS: the server requires and verifies a client certificate
/// signed by `client_ca_pem`, not just presents its own.
pub fn server_tls_config(
    server_cert_pem: &[u8],
    server_key_pem: &[u8],
    client_ca_pem: &[u8],
) -> ServerTlsConfig {
    let identity = Identity::from_pem(server_cert_pem, server_key_pem);
    let client_ca = Certificate::from_pem(client_ca_pem);
    ServerTlsConfig::new()
        .identity(identity)
        .client_ca_root(client_ca)
}

/// `domain` must match a SAN on `server_ca_pem`'s issued server
/// certificate — rustls verifies the server's presented cert against it.
pub fn client_tls_config(
    client_cert_pem: &[u8],
    client_key_pem: &[u8],
    server_ca_pem: &[u8],
    domain: &str,
) -> ClientTlsConfig {
    let identity = Identity::from_pem(client_cert_pem, client_key_pem);
    let server_ca = Certificate::from_pem(server_ca_pem);
    ClientTlsConfig::new()
        .domain_name(domain)
        .ca_certificate(server_ca)
        .identity(identity)
}
