//! Push-mode tests for `NodeBridge` — the client-side half of a capability
//! `conflux-net`'s server side has had.
//!
//! Same shape as `integration.rs`'s pull-mode tests: a fake
//! `RoundDispatcher` stands in for the real `conflux-server`, and a real
//! `conflux-net` transport stands in for the Python `ClientApp`, connecting
//! to `conflux-node`'s real local server. Two real gRPC hops, both carrying
//! a real server-streaming RPC — not a mocked stream handed straight to the
//! assertion.
//!
//! What's specific to push mode is that the thing under test is a
//! *subscription*, so these tests cover what a single request/response call
//! never has to: what happens when the upstream stream drops, and when it
//! keeps dropping.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use conflux_net::tls::{client_tls_config, server_tls_config};
use conflux_net::{
    DispatchError, FlTransportService, PullTransport, PushTransport, RoundDispatcher, TaskStream,
};
use conflux_node::{ConnectionMode, NodeBridge};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{
    DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse, decode_weights,
    encode_weights,
};
use rcgen::{CertificateParams, DistinguishedName, DnType, Issuer, KeyPair, SanType};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};

fn task(round: u64, weights: &[f32]) -> TaskResponse {
    TaskResponse {
        task_id: format!("round-{round}"),
        round,
        model_weights: encode_weights(weights),
        ..Default::default()
    }
}

/// Builds a stream that yields `task` and then *stays open*, the way a
/// real server's subscription sits idle between rounds rather than closing
/// after each one.
///
/// The spawned task parks forever on purpose: holding the sender is what
/// keeps the stream live. It's cleaned up when the test process exits —
/// acceptable here, and much closer to real push-mode behavior than a
/// stream that ends the moment it has delivered something (which would
/// make every test below accidentally exercise the reconnect path).
fn open_stream_yielding(task: TaskResponse) -> TaskStream {
    let (tx, rx) = mpsc::channel(4);
    tokio::spawn(async move {
        let _ = tx.send(Ok(task)).await;
        std::future::pending::<()>().await;
    });
    Box::pin(ReceiverStream::new(rx))
}

/// Stands in for `conflux-server` in push mode: pushes one task down every
/// subscription, records whatever `submit_delta` forwards back.
struct PushUpstream {
    task: TaskResponse,
    received: Arc<StdMutex<Vec<DeltaChunk>>>,
    fetch_task_calls: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl RoundDispatcher for PushUpstream {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        // Counted rather than `unreachable!` so the E2E test can *prove*
        // the task arrived by push, instead of merely assuming it.
        self.fetch_task_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.task.clone())
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        Ok(open_stream_yielding(self.task.clone()))
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        self.received.lock().expect("mutex poisoned").extend(chunks);
        Ok(SubmitAck {
            accepted: true,
            message: "ok".to_string(),
        })
    }

    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            message: String::new(),
        })
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse { acknowledged: true })
    }
}

/// Closes the first `close_immediately_for` subscriptions without sending
/// anything, then behaves normally — a server that accepts a subscription
/// and drops it, which is the failure push mode has to survive and pull
/// mode has no equivalent of.
struct FlakyPushUpstream {
    subscribe_calls: Arc<AtomicU32>,
    close_immediately_for: u32,
    task: TaskResponse,
}

#[async_trait::async_trait]
impl RoundDispatcher for FlakyPushUpstream {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        unreachable!("push-mode test never calls fetch_task")
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        let call = self.subscribe_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call <= self.close_immediately_for {
            return Ok(Box::pin(tokio_stream::empty()));
        }
        Ok(open_stream_yielding(self.task.clone()))
    }

    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        unreachable!("not exercised by the reconnect tests")
    }

    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            message: String::new(),
        })
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse { acknowledged: true })
    }
}

async fn spawn_grpc<D: RoundDispatcher>(dispatcher: D) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(Arc::new(
                dispatcher,
            ))))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

/// Serves `bridge` on a fresh loopback port — `conflux-node`'s own local
/// listener, the one a Python `ClientApp` connects to.
async fn spawn_local_hop(bridge: Arc<NodeBridge>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn push_mode_delivers_a_task_over_two_hops_and_forwards_the_submission_back() {
    let received = Arc::new(StdMutex::new(Vec::new()));
    let fetch_task_calls = Arc::new(AtomicU32::new(0));
    let upstream_addr = spawn_grpc(PushUpstream {
        task: task(1, &[1.0, 2.0]),
        received: Arc::clone(&received),
        fetch_task_calls: Arc::clone(&fetch_task_calls),
    })
    .await;

    let upstream = PushTransport::connect(upstream_addr).await.unwrap();
    let bridge = Arc::new(NodeBridge::new_push(upstream, "node-1".to_string()));
    assert_eq!(bridge.connection_mode().await, ConnectionMode::Push);
    let local_addr = spawn_local_hop(bridge).await;

    // Stands in for the Python ClientApp, subscribing to conflux-node.
    let mut python_stub = PushTransport::connect(local_addr).await.unwrap();
    let mut stream = python_stub.subscribe_tasks("py-client").await.unwrap();

    let pushed = stream.message().await.unwrap().expect("a task was pushed");
    assert_eq!(pushed.round, 1);
    assert_eq!(
        decode_weights(&pushed.model_weights).unwrap(),
        vec![1.0, 2.0]
    );

    let ack = python_stub
        .submit_delta(vec![DeltaChunk {
            client_id: "py-client".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[2.0, 3.0]),
            num_samples: 42,
            ..Default::default()
        }])
        .await
        .unwrap();
    assert!(ack.accepted);

    let forwarded = received.lock().unwrap();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(decode_weights(&forwarded[0].data).unwrap(), vec![2.0, 3.0]);
    assert_eq!(forwarded[0].num_samples, 42);

    // The whole point of push mode: the task arrived without anyone ever
    // asking for it.
    assert_eq!(fetch_task_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_dropped_upstream_subscription_is_re_established_and_still_delivers() {
    let subscribe_calls = Arc::new(AtomicU32::new(0));
    let upstream_addr = spawn_grpc(FlakyPushUpstream {
        subscribe_calls: Arc::clone(&subscribe_calls),
        close_immediately_for: 2, // two dropped subscriptions, then a good one
        task: task(7, &[9.0]),
    })
    .await;

    let upstream = PushTransport::connect(upstream_addr).await.unwrap();
    let bridge = Arc::new(NodeBridge::new_push(upstream, "node-1".to_string()));
    let local_addr = spawn_local_hop(bridge).await;

    let mut python_stub = PushTransport::connect(local_addr).await.unwrap();
    let mut stream = python_stub.subscribe_tasks("py-client").await.unwrap();

    // The local subscriber sees a delay, not an error — reconnection is
    // invisible from this side of the bridge.
    let pushed = stream
        .message()
        .await
        .unwrap()
        .expect("the task from the third subscription should arrive");
    assert_eq!(pushed.round, 7);
    assert_eq!(decode_weights(&pushed.model_weights).unwrap(), vec![9.0]);
    assert_eq!(subscribe_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_server_that_keeps_closing_subscriptions_surfaces_an_error_instead_of_stalling() {
    let subscribe_calls = Arc::new(AtomicU32::new(0));
    let upstream_addr = spawn_grpc(FlakyPushUpstream {
        subscribe_calls: Arc::clone(&subscribe_calls),
        close_immediately_for: u32::MAX, // never delivers anything
        task: task(1, &[]),
    })
    .await;

    let upstream = PushTransport::connect(upstream_addr).await.unwrap();
    let bridge = Arc::new(NodeBridge::new_push(upstream, "node-1".to_string()));
    let local_addr = spawn_local_hop(bridge).await;

    let mut python_stub = PushTransport::connect(local_addr).await.unwrap();
    let mut stream = python_stub.subscribe_tasks("py-client").await.unwrap();

    let started = Instant::now();
    let err = stream
        .message()
        .await
        .expect_err("giving up should surface as an error, not a silent end-of-stream");
    let elapsed = started.elapsed();

    assert!(
        err.message().contains("without delivering a task"),
        "unexpected error message: {}",
        err.message()
    );
    // Three attempts, and the same 50ms-doubling backoff shape
    // `fetch_task`'s retry loop uses: 50ms after the first failure, 100ms
    // after the second, then give up — so at least 150ms must have passed.
    assert_eq!(subscribe_calls.load(Ordering::SeqCst), 3);
    assert!(
        elapsed >= Duration::from_millis(150),
        "expected the shared 50ms-doubling backoff between attempts, took only {elapsed:?}"
    );
}

#[tokio::test]
async fn each_mode_explains_itself_when_asked_for_the_other_modes_rpc() {
    // Push-mode node asked for a pull-mode task.
    let upstream_addr = spawn_grpc(PushUpstream {
        task: task(1, &[]),
        received: Arc::new(StdMutex::new(Vec::new())),
        fetch_task_calls: Arc::new(AtomicU32::new(0)),
    })
    .await;
    let push_bridge = NodeBridge::new_push(
        PushTransport::connect(upstream_addr.clone()).await.unwrap(),
        "node-1".to_string(),
    );
    let err = RoundDispatcher::fetch_task(&push_bridge, "py-client")
        .await
        .expect_err("fetch_task has no meaning against a push upstream");
    let msg = err.to_string();
    assert!(msg.contains("push mode"), "unexpected message: {msg}");
    assert!(msg.contains("subscribe_tasks"), "unexpected message: {msg}");

    // Pull-mode node asked for a push-mode subscription.
    let pull_bridge = NodeBridge::new(
        PullTransport::connect(upstream_addr).await.unwrap(),
        "node-1".to_string(),
    );
    assert_eq!(pull_bridge.connection_mode().await, ConnectionMode::Pull);
    // Matched rather than `expect_err`-ed: the success type is a boxed
    // trait object stream, which has no `Debug` to unwrap through.
    let err = match RoundDispatcher::subscribe_tasks(&pull_bridge, "py-client").await {
        Ok(_) => panic!("subscribe_tasks has no meaning against a pull upstream"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(msg.contains("pull mode"), "unexpected message: {msg}");
    assert!(msg.contains("fetch_task"), "unexpected message: {msg}");
}

// --- cross_silo's actual default posture: push + mTLS, together ---------

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

fn issue_leaf(ca: &GeneratedCa, common_name: &str, san_dns: &str) -> (String, String) {
    let mut params = CertificateParams::new(vec![san_dns.to_string()]).unwrap();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, common_name);
    params.distinguished_name = dn;
    params.subject_alt_names = vec![SanType::DnsName(san_dns.try_into().unwrap())];

    let key_pair = KeyPair::generate().unwrap();
    let cert = params.signed_by(&key_pair, &ca.issuer).unwrap();
    (cert.pem(), key_pair.serialize_pem())
}

/// `cross_silo`'s topology defaults are `push` + `mtls`. Until now those
/// two were only ever tested apart — mTLS against `register` in
/// `conflux-net`'s own suite, push against a plaintext hop above — so
/// nothing actually proved the framework's own default configuration
/// works end to end. This is that proof.
#[tokio::test]
async fn cross_silo_defaults_push_over_mtls_deliver_a_task_end_to_end() {
    let server_ca = make_ca("conflux-test-server-ca-push");
    let client_ca = make_ca("conflux-test-client-ca-push");
    let (server_cert, server_key) = issue_leaf(&server_ca, "conflux-server", "localhost");
    let (client_cert, client_key) = issue_leaf(&client_ca, "conflux-node", "conflux-node");

    // The network hop: a real mTLS-required conflux-server stand-in.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let tls = server_tls_config(
        server_cert.as_bytes(),
        server_key.as_bytes(),
        client_ca.cert_pem.as_bytes(),
    );
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .tls_config(tls)
            .unwrap()
            .add_service(FlTransportServer::new(FlTransportService::new(Arc::new(
                PushUpstream {
                    task: task(3, &[0.5, 1.5]),
                    received: Arc::new(StdMutex::new(Vec::new())),
                    fetch_task_calls: Arc::new(AtomicU32::new(0)),
                },
            ))))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let mut upstream = PushTransport::connect_with_tls(
        format!("https://{addr}"),
        client_tls_config(
            client_cert.as_bytes(),
            client_key.as_bytes(),
            server_ca.cert_pem.as_bytes(),
            "localhost",
        ),
    )
    .await
    .expect("mTLS handshake to the push-mode server should succeed");

    // Registration goes over the same mutually-authenticated connection
    // the subscription will use, exactly as `main.rs` does it.
    let registered = upstream
        .register("node-1", "node-auth-token")
        .await
        .unwrap();
    assert!(registered.accepted);

    // The local hop stays plaintext loopback — ADR 0004: mTLS secures the
    // network hop, not the localhost one.
    let bridge = Arc::new(NodeBridge::new_push(upstream, "node-1".to_string()));
    let local_addr = spawn_local_hop(bridge).await;

    let mut python_stub = PushTransport::connect(local_addr).await.unwrap();
    let mut stream = python_stub.subscribe_tasks("py-client").await.unwrap();
    let pushed = stream
        .message()
        .await
        .unwrap()
        .expect("a task should be pushed over the mTLS connection");
    assert_eq!(pushed.round, 3);
    assert_eq!(
        decode_weights(&pushed.model_weights).unwrap(),
        vec![0.5, 1.5]
    );
}
