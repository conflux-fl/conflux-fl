//! Real end-to-end JWT verification tests — proving that
//! `auth = jwt` actually gates the `Register` RPC over a live gRPC
//! connection, not just that the verifier works in isolation.
//!
//! Same rigor and shape as `node_auth.rs` does for the
//! allow-list: a real `AppState`, a real `FlTransportService`, a real
//! `PullTransport` client, and real ES256 keys generated per test rather
//! than a private key committed to the repository.
//!
//! The point these tests exist to pin down is that **JWT verification
//! and the allow-list are independent gates**. It is easy to write an
//! implementation where a valid token implicitly authorizes, or where
//! being on the allow-list short-circuits token checking; either would
//! pass a test that only ever exercises one of them at a time. The last
//! two tests here exercise both at once, in both directions.

use std::sync::Arc;

use conflux_config::{AuthMode, Mode, Overrides, Topology};
use conflux_net::FlTransportService;
use conflux_net::PullTransport;
use conflux_net::jwt::JwtKeyMaterial;
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_registry::{ClientId, NodeAllowlist, NodeIdentity};
use conflux_server::AppState;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rcgen::KeyPair;
use serde::Serialize;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

#[derive(Serialize)]
struct TestClaims {
    sub: String,
    exp: u64,
    iat: u64,
}

/// An ES256 issuer: the public half becomes the server's verification
/// key, the private half signs tokens. `rcgen::KeyPair::generate()` is
/// ECDSA P-256 — ES256's curve — so no key material is checked in.
struct Issuer {
    public_pem: String,
    encoding: EncodingKey,
}

fn issuer() -> Issuer {
    let key_pair = KeyPair::generate().unwrap();
    Issuer {
        public_pem: key_pair.public_key_pem(),
        encoding: EncodingKey::from_ec_pem(key_pair.serialize_pem().as_bytes()).unwrap(),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

impl Issuer {
    fn token_for(&self, sub: &str) -> String {
        self.sign(sub, now() + 3600)
    }

    fn expired_token_for(&self, sub: &str) -> String {
        // Two hours past, well beyond `Validation`'s 60s default leeway.
        self.sign(sub, now() - 7200)
    }

    fn sign(&self, sub: &str, exp: u64) -> String {
        encode(
            &Header::new(Algorithm::ES256),
            &TestClaims {
                sub: sub.to_string(),
                exp,
                iat: now(),
            },
            &self.encoding,
        )
        .unwrap()
    }

    fn key_material(&self) -> JwtKeyMaterial {
        JwtKeyMaterial::from_public_key_pem(self.public_pem.as_bytes()).unwrap()
    }
}

/// `cross_device`'s topology default is already `auth = jwt`, so this is
/// the framework's own default posture for three of four topologies —
/// not a contrived override.
fn jwt_config(require_node_auth: bool) -> conflux_config::ResolvedConfig {
    let overrides = Overrides {
        require_node_auth: Some(require_node_auth),
        ..Default::default()
    };
    let config = conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        None,
        &Overrides::default(),
        &overrides,
    )
    .unwrap();
    assert_eq!(
        config.auth.value,
        AuthMode::Jwt,
        "cross_device defaults to jwt"
    );
    config
}

async fn spawn(state: Arc<AppState>) -> String {
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

/// Flips one character of a token's signature segment, leaving the
/// header and payload byte-identical — the tamper a signature check
/// exists to catch.
fn tamper(token: &str) -> String {
    let (body, signature) = token.rsplit_once('.').unwrap();
    let mut chars: Vec<char> = signature.chars().collect();
    chars[0] = if chars[0] == 'A' { 'B' } else { 'A' };
    format!("{body}.{}", chars.into_iter().collect::<String>())
}

#[tokio::test]
async fn a_validly_signed_token_registers_over_a_real_connection() {
    let issuer = issuer();
    let state = Arc::new(
        AppState::new(jwt_config(false), vec![0.0]).with_jwt_key(Some(issuer.key_material())),
    );
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    let response = client
        .register("node-1", &issuer.token_for("node-1"))
        .await
        .expect("a validly signed token should register");

    assert!(response.accepted);
}

#[tokio::test]
async fn a_tampered_token_is_rejected_as_unauthenticated_not_permission_denied() {
    let issuer = issuer();
    let state = Arc::new(
        AppState::new(jwt_config(false), vec![0.0]).with_jwt_key(Some(issuer.key_material())),
    );
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    let err = client
        .register("node-1", &tamper(&issuer.token_for("node-1")))
        .await
        .expect_err("a tampered token must not register");

    // The distinction that matters to an operator reading logs: a bad
    // credential (16 Unauthenticated) is not the same event as a good
    // credential that isn't on the guest list (7 PermissionDenied).
    let conflux_net::TransportError::Rpc(status) = err else {
        panic!("expected an RPC status, got {err:?}");
    };
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status:?}");
    assert!(
        status.message().contains("signature"),
        "the rejection should say what was wrong: {}",
        status.message()
    );
}

#[tokio::test]
async fn an_expired_token_is_rejected_over_a_real_connection() {
    let issuer = issuer();
    let state = Arc::new(
        AppState::new(jwt_config(false), vec![0.0]).with_jwt_key(Some(issuer.key_material())),
    );
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    let err = client
        .register("node-1", &issuer.expired_token_for("node-1"))
        .await
        .expect_err("an expired token must not register");

    let conflux_net::TransportError::Rpc(status) = err else {
        panic!("expected an RPC status, got {err:?}");
    };
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
    assert!(status.message().contains("expired"), "{}", status.message());
}

#[tokio::test]
async fn a_token_issued_for_another_client_cannot_register_this_one() {
    let issuer = issuer();
    let state = Arc::new(
        AppState::new(jwt_config(false), vec![0.0]).with_jwt_key(Some(issuer.key_material())),
    );
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    // Genuine signature, unexpired — only the subject is someone else.
    let err = client
        .register("node-2", &issuer.token_for("node-1"))
        .await
        .expect_err("node-1's token must not register node-2");

    let conflux_net::TransportError::Rpc(status) = err else {
        panic!("expected an RPC status, got {err:?}");
    };
    assert_eq!(status.code(), tonic::Code::Unauthenticated);
}

// --- the two gates are genuinely independent ------------------------

#[tokio::test]
async fn a_valid_token_still_loses_to_an_allowlist_that_does_not_include_the_client() {
    let issuer = issuer();
    let state = Arc::new(
        AppState::new(jwt_config(true), vec![0.0]).with_jwt_key(Some(issuer.key_material())),
    );
    // Deliberately allow a *different* client — node-1 authenticates
    // successfully and is still not invited.
    state
        .node_allowlist
        .allow(
            ClientId("someone-else".to_string()),
            NodeIdentity::SharedToken("whatever".to_string()),
        )
        .await
        .unwrap();
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    let err = client
        .register("node-1", &issuer.token_for("node-1"))
        .await
        .expect_err("a valid token does not put a client on the allow-list");

    let conflux_net::TransportError::Rpc(status) = err else {
        panic!("expected an RPC status, got {err:?}");
    };
    // PermissionDenied, not Unauthenticated: the token was fine.
    assert_eq!(status.code(), tonic::Code::PermissionDenied, "{status:?}");
}

#[tokio::test]
async fn being_on_the_allowlist_does_not_excuse_a_bad_token() {
    let issuer = issuer();
    let state = Arc::new(
        AppState::new(jwt_config(true), vec![0.0]).with_jwt_key(Some(issuer.key_material())),
    );
    let expired = issuer.expired_token_for("node-1");
    // node-1 is on the allow-list under the exact token it will present,
    // so the allow-list check would pass — the JWT check is what stops
    // it, and it runs first.
    state
        .node_allowlist
        .allow(
            ClientId("node-1".to_string()),
            NodeIdentity::SharedToken(expired.clone()),
        )
        .await
        .unwrap();
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    let err = client
        .register("node-1", &expired)
        .await
        .expect_err("an expired token is refused even for an allow-listed client");

    let conflux_net::TransportError::Rpc(status) = err else {
        panic!("expected an RPC status, got {err:?}");
    };
    assert_eq!(status.code(), tonic::Code::Unauthenticated, "{status:?}");
}

#[tokio::test]
async fn research_mode_without_a_key_still_registers_anyone() {
    // Today's behavior before this phase, preserved: research mode with
    // `auth = jwt` and no key configured verifies nothing. Asserted
    // rather than assumed, because this is the permissive path — the one
    // where a regression is silent.
    let state = Arc::new(AppState::new(jwt_config(false), vec![0.0]));
    let addr = spawn(state).await;

    let mut client = PullTransport::connect(addr).await.unwrap();
    let response = client
        .register("node-1", "not-even-a-jwt")
        .await
        .expect("research mode with no key configured verifies nothing");

    assert!(response.accepted);
}
