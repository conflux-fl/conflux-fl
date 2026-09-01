//! Real mTLS handshake tests: a client presenting a cert signed by the
//! configured CA connects and completes an RPC; a client presenting a
//! cert from an untrusted CA is rejected at the TLS layer, before any RPC
//! logic runs.

use std::sync::{Arc, Mutex};

use conflux_net::tls::{client_tls_config, server_tls_config};
use conflux_net::{DispatchError, FlTransportService, PullTransport, RoundDispatcher, TaskStream};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

struct AcceptAllDispatcher;

#[async_trait::async_trait]
impl RoundDispatcher for AcceptAllDispatcher {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        unreachable!("not exercised by the mTLS tests")
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        unreachable!("not exercised by the mTLS tests")
    }

    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        unreachable!("not exercised by the mTLS tests")
    }

    async fn register(
        &self,
        client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            message: format!("welcome {client_id}"),
        })
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        unreachable!("not exercised by the mTLS tests")
    }
}

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

/// Issues a leaf cert (server or client) signed by `ca`.
fn issue_leaf(ca: &GeneratedCa, common_name: &str, san_dns: &str) -> (String, String) {
    issue_leaf_with_der(ca, common_name, san_dns).0
}

/// Like [`issue_leaf`], but also hands back the DER bytes — needed by
/// `register_delivers_the_callers_cert_fingerprint_to_the_dispatcher` to
/// independently compute the fingerprint `peer_cert_fingerprint` should
/// have extracted.
fn issue_leaf_with_der(
    ca: &GeneratedCa,
    common_name: &str,
    san_dns: &str,
) -> ((String, String), CertificateDer<'static>) {
    let mut params = CertificateParams::new(vec![san_dns.to_string()]).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    params.subject_alt_names = vec![SanType::DnsName(san_dns.try_into().unwrap())];

    let key_pair = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key_pair, &ca.issuer).unwrap();
    let der = cert.der().clone();
    ((cert.pem(), key_pair.serialize_pem()), der)
}

/// Serves `FlTransportService` with mTLS required (client certs must be
/// signed by `client_ca_pem`), returning the address to connect to.
async fn spawn_mtls_server(
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
            .add_service(FlTransportServer::new(FlTransportService::new(Arc::new(
                AcceptAllDispatcher,
            ))))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    format!("https://{addr}")
}

/// Records whatever `peer_cert_fingerprint` `service.rs` extracted and
/// handed it, so the test can compare it against an independently
/// computed expectation.
struct FingerprintCapturingDispatcher {
    captured: Arc<Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl RoundDispatcher for FingerprintCapturingDispatcher {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        unreachable!("not exercised by this test")
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        unreachable!("not exercised by this test")
    }

    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        unreachable!("not exercised by this test")
    }

    async fn register(
        &self,
        client_id: &str,
        _auth_token: &str,
        peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        *self.captured.lock().unwrap() = peer_cert_fingerprint.map(str::to_string);
        Ok(RegisterResponse {
            accepted: true,
            message: format!("welcome {client_id}"),
        })
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        unreachable!("not exercised by this test")
    }
}

#[tokio::test]
async fn register_delivers_the_callers_cert_fingerprint_to_the_dispatcher() {
    let server_ca = make_ca("conflux-test-server-ca-fp");
    let client_ca = make_ca("conflux-test-client-ca-fp");

    let ((server_cert, server_key), _server_der) =
        issue_leaf_with_der(&server_ca, "conflux-server", "localhost");
    let ((client_cert, client_key), client_der) =
        issue_leaf_with_der(&client_ca, "conflux-client", "conflux-client");

    let expected_fingerprint: String = {
        let mut hasher = Sha256::new();
        hasher.update(client_der.as_ref());
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let tls = server_tls_config(
        server_cert.as_bytes(),
        server_key.as_bytes(),
        client_ca.cert_pem.as_bytes(),
    );
    let captured = Arc::new(Mutex::new(None));
    let dispatcher = Arc::new(FingerprintCapturingDispatcher {
        captured: Arc::clone(&captured),
    });
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(FlTransportServer::new(FlTransportService::new(dispatcher)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let addr = format!("https://{addr}");

    let client_tls = client_tls_config(
        client_cert.as_bytes(),
        client_key.as_bytes(),
        server_ca.cert_pem.as_bytes(),
        "localhost",
    );
    let mut transport = PullTransport::connect_with_tls(addr, client_tls)
        .await
        .expect("trusted client cert should be accepted");

    let response = transport.register("client-1", "token").await.unwrap();
    assert!(response.accepted);

    assert_eq!(
        captured.lock().unwrap().as_deref(),
        Some(expected_fingerprint.as_str()),
        "the dispatcher must receive the SHA-256 fingerprint of the \
         client's actual leaf certificate, matching what an independent \
         hash of its DER bytes produces"
    );
}

#[tokio::test]
async fn client_with_cert_from_trusted_ca_connects_and_completes_an_rpc() {
    let server_ca = make_ca("conflux-test-server-ca");
    let client_ca = make_ca("conflux-test-client-ca");

    let (server_cert, server_key) = issue_leaf(&server_ca, "conflux-server", "localhost");
    let (client_cert, client_key) = issue_leaf(&client_ca, "conflux-client", "conflux-client");

    let addr = spawn_mtls_server(&server_cert, &server_key, &client_ca.cert_pem).await;

    let client_tls = client_tls_config(
        client_cert.as_bytes(),
        client_key.as_bytes(),
        server_ca.cert_pem.as_bytes(),
        "localhost",
    );
    let mut transport = PullTransport::connect_with_tls(addr, client_tls)
        .await
        .expect("trusted client cert should be accepted");

    let response = transport.register("client-1", "token").await.unwrap();
    assert!(response.accepted);
}

#[tokio::test]
async fn client_with_cert_from_untrusted_ca_is_rejected_at_the_tls_layer() {
    let server_ca = make_ca("conflux-test-server-ca-2");
    let trusted_client_ca = make_ca("conflux-test-trusted-client-ca");
    let untrusted_client_ca = make_ca("conflux-test-untrusted-client-ca");

    let (server_cert, server_key) = issue_leaf(&server_ca, "conflux-server", "localhost");
    // Signed by a CA the server does NOT trust.
    let (bad_client_cert, bad_client_key) = issue_leaf(
        &untrusted_client_ca,
        "conflux-bad-client",
        "conflux-bad-client",
    );

    let addr = spawn_mtls_server(&server_cert, &server_key, &trusted_client_ca.cert_pem).await;

    let client_tls = client_tls_config(
        bad_client_cert.as_bytes(),
        bad_client_key.as_bytes(),
        server_ca.cert_pem.as_bytes(),
        "localhost",
    );

    // The actual TLS handshake happens lazily — tonic's `connect()` can
    // return `Ok` before the handshake has run to completion, so the
    // rejection surfaces on the first real RPC rather than necessarily at
    // `connect()` itself. Either outcome proves the same thing: this
    // client never gets a successful RPC through.
    let result = PullTransport::connect_with_tls(addr, client_tls).await;
    let Ok(mut transport) = result else {
        return; // connect() itself failing is an acceptable rejection too
    };
    let rpc_result = transport.register("client-1", "token").await;
    assert!(
        rpc_result.is_err(),
        "a client cert signed by an untrusted CA must be rejected — either at \
         connect() or on the first RPC, but a client-1 registration must never \
         succeed against a server that doesn't trust this CA"
    );
}

#[tokio::test]
async fn plaintext_connection_is_rejected_by_an_mtls_required_server() {
    let server_ca = make_ca("conflux-test-server-ca-3");
    let client_ca = make_ca("conflux-test-client-ca-3");
    let (server_cert, server_key) = issue_leaf(&server_ca, "conflux-server", "localhost");

    let addr = spawn_mtls_server(&server_cert, &server_key, &client_ca.cert_pem).await;

    // No TLS at all — the server must not silently accept a downgraded,
    // unencrypted connection just because a plaintext client asked.
    let result = PullTransport::connect(addr).await;
    let Ok(mut transport) = result else {
        return; // connect() itself failing is an acceptable rejection too
    };
    let rpc_result = transport.register("client-1", "token").await;
    assert!(
        rpc_result.is_err(),
        "a plaintext client must not be able to complete an RPC against an mTLS-required server"
    );
}
