//! Hermetic (Rust-only) integration tests for `NodeBridge`. A fake
//! `RoundDispatcher` stands in for the real `conflux-server`; a real
//! `conflux-net::PullTransport` stands in for the Python `ClientApp`,
//! connecting to `conflux-node`'s real local server — proving the
//! forwarding logic actually works, not just that the two halves compile.
//!
//! The real, cross-language, cross-process verification (actual
//! `conflux-server` + actual `conflux-node` + the actual
//! `stub_client.py`) is a manual smoke test recorded in `docs/STATUS.md` —
//! spawning arbitrary external processes inside `cargo test` isn't
//! hermetic or standard practice.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use conflux_net::{DispatchError, FlTransportService, PullTransport, RoundDispatcher, TaskStream};
use conflux_node::NodeBridge;
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{
    DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse, decode_weights,
    encode_weights,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Stands in for `conflux-server`: hands out a fixed task, records
/// whatever `submit_delta` forwards to it.
struct FakeUpstream {
    task: TaskResponse,
    received: Arc<StdMutex<Vec<DeltaChunk>>>,
}

#[async_trait::async_trait]
impl RoundDispatcher for FakeUpstream {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        Ok(self.task.clone())
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        Ok(Box::pin(tokio_stream::empty()))
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

/// Fails `fetch_task` for the first `fail_until_attempt` calls, then
/// succeeds — for exercising `NodeBridge`'s retry/backoff.
struct FlakyUpstream {
    attempts: AtomicU32,
    fail_until_attempt: u32,
    task: TaskResponse,
}

#[async_trait::async_trait]
impl RoundDispatcher for FlakyUpstream {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        let attempt = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt <= self.fail_until_attempt {
            return Err(DispatchError::Other(format!(
                "simulated upstream failure on attempt {attempt}"
            )));
        }
        Ok(self.task.clone())
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        Ok(Box::pin(tokio_stream::empty()))
    }

    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        unreachable!("not exercised by the retry test")
    }

    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        unreachable!("not exercised by the retry test")
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        unreachable!("not exercised by the retry test")
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

#[tokio::test]
async fn local_hop_forwards_fetch_task_and_submit_delta_to_upstream() {
    let received = Arc::new(StdMutex::new(Vec::new()));
    let fake_upstream = FakeUpstream {
        task: TaskResponse {
            task_id: "round-1".to_string(),
            round: 1,
            model_weights: encode_weights(&[1.0, 2.0]),
            ..Default::default()
        },
        received: Arc::clone(&received),
    };
    let upstream_addr = spawn_grpc(fake_upstream).await;

    let upstream_transport = PullTransport::connect(upstream_addr).await.unwrap();
    let bridge = Arc::new(NodeBridge::new(upstream_transport, "node-1".to_string()));

    let local_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local_listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
            .serve_with_incoming(TcpListenerStream::new(local_listener))
            .await
            .unwrap();
    });

    // Stands in for the Python ClientApp connecting to conflux-node.
    let mut python_stub = PullTransport::connect(format!("http://{local_addr}"))
        .await
        .unwrap();

    let task = python_stub.fetch_task("py-client").await.unwrap();
    assert_eq!(task.round, 1);
    assert_eq!(decode_weights(&task.model_weights).unwrap(), vec![1.0, 2.0]);

    let trained = encode_weights(&[2.0, 3.0]); // dummy "training": +1.0 to every weight
    let ack = python_stub
        .submit_delta(vec![DeltaChunk {
            client_id: "py-client".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: trained,
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
}

#[tokio::test]
async fn fetch_task_retries_through_transient_upstream_failures() {
    let flaky = FlakyUpstream {
        attempts: AtomicU32::new(0),
        fail_until_attempt: 2, // fails twice, succeeds on the 3rd — within NodeBridge's budget
        task: TaskResponse {
            task_id: "round-5".to_string(),
            round: 5,
            model_weights: vec![],
            ..Default::default()
        },
    };
    let upstream_addr = spawn_grpc(flaky).await;

    let upstream_transport = PullTransport::connect(upstream_addr).await.unwrap();
    let bridge = NodeBridge::new(upstream_transport, "node-1".to_string());

    let task = RoundDispatcher::fetch_task(&bridge, "py-client")
        .await
        .expect("retry should have recovered from the transient failures");
    assert_eq!(task.round, 5);
}
