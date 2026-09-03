//! Real end-to-end tests: `resolve_server_tls`'s `Some(ServerTlsConfig)`
//! actually binds a working mTLS server, and `None` binds a working
//! plaintext one — not just that the decision function type-checks.

use std::sync::Arc;

use conflux_config::{AuthMode, Mode};
use conflux_net::tls::client_tls_config;
use conflux_net::{FlTransportService, PullTransport};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_server::{AppState, TlsMaterial, resolve_server_tls};
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

fn make_ca(common_name: &str) -> (String, Issuer<'static, KeyPair>) {
    let mut params = CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;

    let key_pair = KeyPair::generate().unwrap();
    let cert = params.self_signed(&key_pair).unwrap();
    let cert_pem = cert.pem();
    let issuer = Issuer::new(params, key_pair);
    (cert_pem, issuer)
}

fn issue_leaf(
    ca_issuer: &Issuer<'static, KeyPair>,
    common_name: &str,
    san_dns: &str,
) -> (String, String) {
    let mut params = CertificateParams::new(vec![san_dns.to_string()]).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    params.subject_alt_names = vec![SanType::DnsName(san_dns.try_into().unwrap())];

    let key_pair = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key_pair, ca_issuer).unwrap();
    (cert.pem(), key_pair.serialize_pem())
}

#[tokio::test]
async fn mtls_with_real_material_binds_a_server_that_requires_a_trusted_client_cert() {
    let (server_ca_pem, server_ca_issuer) = make_ca("conflux-test-server-ca");
    let (client_ca_pem, client_ca_issuer) = make_ca("conflux-test-client-ca");
    let (server_cert, server_key) = issue_leaf(&server_ca_issuer, "conflux-server", "localhost");
    let (client_cert, client_key) =
        issue_leaf(&client_ca_issuer, "conflux-client", "conflux-client");

    let tls_config = resolve_server_tls(
        Mode::Production,
        AuthMode::Mtls,
        Some(TlsMaterial {
            cert_pem: server_cert.into_bytes(),
            key_pem: server_key.into_bytes(),
            client_ca_pem: client_ca_pem.into_bytes(),
        }),
    )
    .unwrap()
    .expect("Mtls + material must produce a ServerTlsConfig");

    // `AppState`'s own config is research mode here — this test is about
    // `resolve_server_tls`'s decision (exercised above with
    // `Mode::Production` directly), not the `require_node_auth`,
    // which would otherwise reject this test's unregistered client.
    let state = Arc::new(AppState::new(
        conflux_config::resolve(
            conflux_config::Topology::CrossSilo,
            Mode::Research,
            None,
            &conflux_config::Overrides::default(),
            &conflux_config::Overrides::default(),
        )
        .unwrap(),
        vec![0.0],
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls_config)
            .unwrap()
            .add_service(FlTransportServer::new(FlTransportService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let addr = format!("https://{addr}");

    // Trusted client cert: connects and completes a real RPC.
    let trusted_tls = client_tls_config(
        client_cert.as_bytes(),
        client_key.as_bytes(),
        server_ca_pem.as_bytes(),
        "localhost",
    );
    let mut transport = PullTransport::connect_with_tls(addr.clone(), trusted_tls)
        .await
        .expect("trusted client cert should be accepted");
    let response = transport.register("client-1", "token").await.unwrap();
    assert!(response.accepted);

    // Plaintext: must not be able to complete an RPC against this
    // TLS-required server.
    let plaintext_result = PullTransport::connect(addr).await;
    let rpc_result = match plaintext_result {
        Ok(mut transport) => transport.register("client-1", "token").await,
        Err(_) => return, // connect() itself failing is an acceptable rejection too
    };
    assert!(
        rpc_result.is_err(),
        "a plaintext client must not complete an RPC once auth=mtls resolved real TLS material"
    );
}

#[tokio::test]
async fn jwt_auth_binds_a_plaintext_server() {
    let tls_config = resolve_server_tls(Mode::Research, AuthMode::Jwt, None).unwrap();
    assert!(tls_config.is_none());

    let state = Arc::new(AppState::new(
        conflux_config::resolve(
            conflux_config::Topology::CrossDevice,
            Mode::Research,
            None,
            &conflux_config::Overrides::default(),
            &conflux_config::Overrides::default(),
        )
        .unwrap(),
        vec![0.0],
    ));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let mut transport = PullTransport::connect(format!("http://{addr}"))
        .await
        .unwrap();
    let response = transport.register("client-1", "token").await.unwrap();
    assert!(response.accepted);
}
