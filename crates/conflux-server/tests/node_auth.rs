//! Real end-to-end node-auth enforcement tests — proving the
//! allow-list built actually gates the `Register` RPC, not just
//! that its data model works in isolation. See

use std::sync::Arc;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::tls::{client_tls_config, server_tls_config};
use conflux_net::{FlTransportService, PullTransport};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_registry::{ClientId, NodeAllowlist, NodeIdentity};
use conflux_server::AppState;
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

fn config_with_node_auth(required: bool) -> conflux_config::ResolvedConfig {
    let overrides = Overrides {
        require_node_auth: Some(required),
        ..Default::default()
    };
    conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        None,
        &Overrides::default(),
        &overrides,
    )
    .unwrap()
}

async fn spawn_plaintext(state: Arc<AppState>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

fn id(s: &str) -> ClientId {
    ClientId(s.to_string())
}

#[tokio::test]
async fn shared_token_client_on_the_allowlist_registers_successfully() {
    let state = Arc::new(AppState::new(config_with_node_auth(true), vec![0.0]));
    state
        .node_allowlist
        .allow(
            id("client-1"),
            NodeIdentity::SharedToken("secret".to_string()),
        )
        .await
        .unwrap();

    let addr = spawn_plaintext(Arc::clone(&state)).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let response = transport.register("client-1", "secret").await.unwrap();
    assert!(response.accepted);
}

#[tokio::test]
async fn shared_token_client_with_the_wrong_token_is_rejected() {
    let state = Arc::new(AppState::new(config_with_node_auth(true), vec![0.0]));
    state
        .node_allowlist
        .allow(
            id("client-1"),
            NodeIdentity::SharedToken("secret".to_string()),
        )
        .await
        .unwrap();

    let addr = spawn_plaintext(Arc::clone(&state)).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let result = transport.register("client-1", "wrong-token").await;
    assert!(
        result.is_err(),
        "a mismatched shared token must be rejected"
    );
}

#[tokio::test]
async fn client_never_added_to_the_allowlist_is_rejected() {
    let state = Arc::new(AppState::new(config_with_node_auth(true), vec![0.0]));

    let addr = spawn_plaintext(Arc::clone(&state)).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let result = transport.register("ghost", "whatever").await;
    assert!(
        result.is_err(),
        "a client_id with no allow-list entry at all must be rejected"
    );
}

#[tokio::test]
async fn revoke_then_register_fails_even_with_the_originally_correct_token() {
    let state = Arc::new(AppState::new(config_with_node_auth(true), vec![0.0]));
    state
        .node_allowlist
        .allow(
            id("client-1"),
            NodeIdentity::SharedToken("secret".to_string()),
        )
        .await
        .unwrap();
    state.node_allowlist.revoke(&id("client-1")).await.unwrap();

    let addr = spawn_plaintext(Arc::clone(&state)).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let result = transport.register("client-1", "secret").await;
    assert!(result.is_err(), "a revoked client must be rejected");
}

#[tokio::test]
async fn require_node_auth_false_keeps_registration_working_with_no_allowlist_entry() {
    // The default (research mode) — proves the toggle has zero effect
    // when off, not just that it compiles when off. Every pre-Phase-8
    // registration test already exercises this path
    // unmodified; this test makes the "off means off" claim explicit.
    let state = Arc::new(AppState::new(config_with_node_auth(false), vec![0.0]));

    let addr = spawn_plaintext(Arc::clone(&state)).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let response = transport.register("client-1", "whatever").await.unwrap();
    assert!(response.accepted);
}

// --- mTLS-based identity: proves gaps 2 and 3 of the Flower-platform design review
// is actually closed — a cert signed by a trusted CA is no longer
// sufficient on its own, the specific identity must also be allow-listed.

struct GeneratedCa {
    cert_pem: String,
    issuer: Issuer<'static, KeyPair>,
}

fn make_ca(common_name: &str) -> GeneratedCa {
    let mut params = CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;

    let key_pair = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_pem = cert.pem();
    let issuer = Issuer::new(params, key_pair);

    GeneratedCa { cert_pem, issuer }
}

/// Issues a leaf cert signed by `ca`, returning its PEM/key and the raw
/// DER bytes (needed to independently compute the fingerprint this test
/// expects the allow-list to be keyed on).
fn issue_leaf(ca: &GeneratedCa, common_name: &str, san_dns: &str) -> (String, String, Vec<u8>) {
    let mut params = CertificateParams::new(vec![san_dns.to_string()]).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    params.subject_alt_names = vec![SanType::DnsName(san_dns.try_into().unwrap())];

    let key_pair = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key_pair, &ca.issuer).unwrap();
    let der = cert.der().as_ref().to_vec();
    (cert.pem(), key_pair.serialize_pem(), der)
}

fn fingerprint_of(der: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(der);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

async fn spawn_mtls(
    state: Arc<AppState>,
    server_cert_pem: &str,
    server_key_pem: &str,
    client_ca_pem: &str,
) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let tls = server_tls_config(
        server_cert_pem.as_bytes(),
        server_key_pem.as_bytes(),
        client_ca_pem.as_bytes(),
    );
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(FlTransportServer::new(FlTransportService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("https://{addr}")
}

#[tokio::test]
async fn mtls_client_whose_cert_fingerprint_is_allowlisted_registers_successfully() {
    let server_ca = make_ca("conflux-test-server-ca-allowed");
    let client_ca = make_ca("conflux-test-client-ca-allowed");
    let (server_cert, server_key, _) = issue_leaf(&server_ca, "conflux-server", "localhost");
    let (client_cert, client_key, client_der) =
        issue_leaf(&client_ca, "conflux-client", "conflux-client");

    let state = Arc::new(AppState::new(config_with_node_auth(true), vec![0.0]));
    state
        .node_allowlist
        .allow(
            id("client-1"),
            NodeIdentity::CertFingerprint(fingerprint_of(&client_der)),
        )
        .await
        .unwrap();

    let addr = spawn_mtls(
        Arc::clone(&state),
        &server_cert,
        &server_key,
        &client_ca.cert_pem,
    )
    .await;
    let client_tls = client_tls_config(
        client_cert.as_bytes(),
        client_key.as_bytes(),
        server_ca.cert_pem.as_bytes(),
        "localhost",
    );
    let mut transport = PullTransport::connect_with_tls(addr, client_tls)
        .await
        .expect("trusted client cert should be accepted at the TLS layer");

    let response = transport.register("client-1", "unused").await.unwrap();
    assert!(response.accepted);
}

#[tokio::test]
async fn mtls_client_with_a_ca_trusted_cert_but_no_allowlist_entry_is_rejected() {
    // The specific case the Flower-platform design review flagged as missing:
    // CA trust alone (a valid handshake) must not be sufficient — the
    // presented identity also has to be on the allow-list.
    let server_ca = make_ca("conflux-test-server-ca-not-allowed");
    let client_ca = make_ca("conflux-test-client-ca-not-allowed");
    let (server_cert, server_key, _) = issue_leaf(&server_ca, "conflux-server", "localhost");
    let (client_cert, client_key, _client_der) =
        issue_leaf(&client_ca, "conflux-client", "conflux-client");

    // require_node_auth = true, but nothing was ever `allow`-ed.
    let state = Arc::new(AppState::new(config_with_node_auth(true), vec![0.0]));

    let addr = spawn_mtls(
        Arc::clone(&state),
        &server_cert,
        &server_key,
        &client_ca.cert_pem,
    )
    .await;
    let client_tls = client_tls_config(
        client_cert.as_bytes(),
        client_key.as_bytes(),
        server_ca.cert_pem.as_bytes(),
        "localhost",
    );
    let mut transport = PullTransport::connect_with_tls(addr, client_tls)
        .await
        .expect("the TLS handshake itself succeeds — this cert IS signed by a trusted CA");

    let result = transport.register("client-1", "unused").await;
    assert!(
        result.is_err(),
        "CA trust alone must not be enough to register — the cert's fingerprint \
         was never added to the allow-list"
    );
}
