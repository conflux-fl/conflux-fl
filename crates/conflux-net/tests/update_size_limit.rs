//! Tier 5 (H1): `submit_delta` must bound how much of a client's stream it
//! will hold.
//!
//! The defect these tests exist to prevent: `submit_delta` collected the
//! whole client stream into a `Vec` before handing it to the dispatcher,
//! with no cap on how many chunks that could be. gRPC's own message-size
//! limit is per *message*, so a client sending an unbounded number of
//! individually-legal chunks grew the server's heap until the process
//! died — a one-client, remotely-triggerable memory exhaustion.
//!
//! The rule these encode: **the transport may reject a stream, but it must
//! never let one client's stream decide how much memory the server
//! allocates.**

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use conflux_net::{
    DispatchError, FlTransportService, PullTransport, RoundDispatcher, TaskStream, TransportError,
};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Counts how many chunks actually reached the dispatcher, so a test can
/// assert the stream was cut *before* delivery rather than rejected after
/// the whole thing was already buffered.
struct CountingDispatcher {
    chunks_seen: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl RoundDispatcher for CountingDispatcher {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        unreachable!("not exercised by these tests")
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        unreachable!("not exercised by these tests")
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        self.chunks_seen.fetch_add(chunks.len(), Ordering::SeqCst);
        Ok(SubmitAck {
            accepted: true,
            message: "ok".to_string(),
        })
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
        Ok(HeartbeatResponse { acknowledged: true })
    }
}

/// Starts a server with an explicit byte bound. Returns its address and the
/// counter the dispatcher increments.
async fn spawn_server(max_update_bytes: u64) -> (String, Arc<AtomicUsize>) {
    let chunks_seen = Arc::new(AtomicUsize::new(0));
    let dispatcher = Arc::new(CountingDispatcher {
        chunks_seen: Arc::clone(&chunks_seen),
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = FlTransportService::new(dispatcher).with_max_update_bytes(max_update_bytes);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    (format!("http://{addr}"), chunks_seen)
}

fn chunk(index: u32, total: u32, bytes: usize) -> DeltaChunk {
    DeltaChunk {
        client_id: "greedy-client".to_string(),
        round: 1,
        chunk_index: index,
        total_chunks: total,
        data: vec![0u8; bytes],
        num_samples: 10,
    }
}

/// The core case. A client streams far more than the configured bound; the
/// server must refuse rather than buffer it, and must say so as
/// `resource_exhausted` — the gRPC code that tells an honest client this
/// will not succeed on retry, as opposed to `internal`, which says the
/// server is broken.
#[tokio::test]
async fn a_stream_past_the_limit_is_refused_and_never_reaches_the_dispatcher() {
    // 1 KiB budget, 40 chunks of 256 bytes = 10 KiB attempted.
    let (addr, chunks_seen) = spawn_server(1024).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let chunks: Vec<DeltaChunk> = (0..40).map(|i| chunk(i, 40, 256)).collect();
    let result = transport.submit_delta(chunks).await;

    match result {
        Err(TransportError::Rpc(status)) => {
            assert_eq!(
                status.code(),
                tonic::Code::ResourceExhausted,
                "expected resource_exhausted, got {status:?}"
            );
            // The message must name the client and the limit — an operator
            // reading this needs to know whose stream was cut and against
            // what bound, or they cannot tell a misconfigured limit from a
            // misbehaving client.
            let msg = status.message();
            assert!(
                msg.contains("greedy-client"),
                "status should name the client, got: {msg}"
            );
            assert!(
                msg.contains("1024"),
                "status should name the limit, got: {msg}"
            );
        }
        Err(other) => panic!("expected an RPC error, got {other:?}"),
        Ok(ack) => panic!("oversized stream was accepted: {ack:?}"),
    }

    assert_eq!(
        chunks_seen.load(Ordering::SeqCst),
        0,
        "the dispatcher must never see an oversized submission — if it does, \
         the whole stream was buffered first, which is the bug"
    );
}

/// The bound must not be the *only* thing that works: an ordinary
/// submission comfortably under it still has to go through untouched.
/// Without this, "reject everything" would pass the test above.
#[tokio::test]
async fn a_stream_within_the_limit_is_delivered_intact() {
    let (addr, chunks_seen) = spawn_server(1024).await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    // 3 chunks × 100 bytes = 300 bytes, well under the 1 KiB budget.
    let chunks: Vec<DeltaChunk> = (0..3).map(|i| chunk(i, 3, 100)).collect();
    let ack = transport.submit_delta(chunks).await.unwrap();

    assert!(ack.accepted);
    assert_eq!(
        chunks_seen.load(Ordering::SeqCst),
        3,
        "every chunk under the limit should reach the dispatcher"
    );
}

/// The boundary itself. A stream landing exactly on the limit is allowed;
/// one byte past it is not. Encoded because "> limit" and ">= limit" are a
/// one-character difference that no other test here would catch.
#[tokio::test]
async fn the_limit_is_inclusive_at_exactly_the_bound() {
    let (addr, chunks_seen) = spawn_server(512).await;

    let mut transport = PullTransport::connect(addr.clone()).await.unwrap();
    let exact = transport.submit_delta(vec![chunk(0, 1, 512)]).await;
    assert!(
        exact.is_ok(),
        "a stream of exactly max_update_bytes must be accepted, got {exact:?}"
    );
    assert_eq!(chunks_seen.load(Ordering::SeqCst), 1);

    let mut transport = PullTransport::connect(addr).await.unwrap();
    let one_over = transport.submit_delta(vec![chunk(0, 1, 513)]).await;
    assert!(
        one_over.is_err(),
        "one byte past max_update_bytes must be refused"
    );
    assert_eq!(
        chunks_seen.load(Ordering::SeqCst),
        1,
        "the over-limit submission must not have reached the dispatcher"
    );
}

/// A service built without an explicit bound still has one. The failure
/// this guards against is a future refactor making the limit opt-in, which
/// would silently reopen the defect for every caller that uses `new`.
#[test]
fn the_default_bound_is_finite_and_not_zero() {
    assert_eq!(
        conflux_net::DEFAULT_MAX_UPDATE_BYTES,
        256 * 1024 * 1024,
        "the default must stay in step with conflux-config's own \
         max_update_bytes builtin — the two are mirrored, not shared"
    );
}
