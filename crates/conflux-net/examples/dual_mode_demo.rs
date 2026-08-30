//! Runnable "try it" for the crate-deep-dives article on `conflux-net`.
//!
//! Run with:
//!   cargo run --example dual_mode_demo -p conflux-net
//!
//! `conflux-net` is a transport crate — there's nothing meaningful to
//! demonstrate without something on the other end of the wire, so this
//! example spins up a minimal in-process `FlTransportService` (the same
//! pattern `tests/integration.rs` uses: a real tonic server bound to an
//! ephemeral TCP port, backed by a trivial in-memory `RoundDispatcher`) and
//! then drives it with both of this crate's real client transports.
//!
//! It walks through the two RPC shapes this crate's dual-mode design
//! actually rests on:
//!   - `FetchTask` (pull mode): one request, one response. The client asks,
//!     the server answers, done.
//!   - `SubscribeTasks` (push mode) and `SubmitDelta` (both modes): a
//!     stream. The server (or client) can send any number of messages
//!     before the stream ends, and the receiving side reads them one at a
//!     time as they arrive rather than waiting for one big payload.

use std::pin::Pin;
use std::sync::Arc;

use conflux_net::{
    DispatchError, FlTransportService, PullTransport, PushTransport, RoundDispatcher, TaskStream,
    TransportError,
};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// A trivial in-memory dispatcher — enough to answer every RPC plausibly,
/// with no real client registry, buffering, or aggregation behind it. This
/// is exactly the seam `conflux-server`'s `AppState` fills in for real.
struct DemoDispatcher;

#[async_trait::async_trait]
impl RoundDispatcher for DemoDispatcher {
    async fn fetch_task(&self, client_id: &str) -> Result<TaskResponse, DispatchError> {
        if client_id == "unregistered-client" {
            return Err(DispatchError::UnknownClient(client_id.to_string()));
        }
        Ok(TaskResponse {
            task_id: "round-1-task".to_string(),
            round: 1,
            model_weights: conflux_proto::encode_weights(&[1.0, 2.0, 3.0]),
        })
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        // Three tasks pushed one at a time, simulating three rounds worth
        // of work becoming ready while the client stays connected — this
        // is the shape a real `BroadcastStream` off `conflux-buffer` takes,
        // just without the buffer.
        let tasks: Vec<Result<TaskResponse, tonic::Status>> = (1..=3)
            .map(|round| {
                Ok(TaskResponse {
                    task_id: format!("round-{round}-task"),
                    round,
                    model_weights: conflux_proto::encode_weights(&[round as f32]),
                })
            })
            .collect();
        let stream: TaskStream = Box::pin(tokio_stream::iter(tasks)) as Pin<Box<_>>;
        Ok(stream)
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        let total_bytes: usize = chunks.iter().map(|c| c.data.len()).sum();
        Ok(SubmitAck {
            accepted: true,
            message: format!(
                "reassembled {} chunk(s), {} byte(s) of weights",
                chunks.len(),
                total_bytes
            ),
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

/// Binds an ephemeral port and serves `DemoDispatcher` on it, returning the
/// `http://` address to connect to.
async fn spawn_demo_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = FlTransportService::new(Arc::new(DemoDispatcher));
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

#[tokio::main]
async fn main() {
    let addr = spawn_demo_server().await;

    // --- Pull mode: request/response, plus client-streaming submission ---
    println!("== PullTransport (request/response) ==");
    let mut pull = PullTransport::connect(&addr).await.unwrap();

    let register = pull.register("client-1", "token").await.unwrap();
    println!(
        "register -> accepted={}, {:?}",
        register.accepted, register.message
    );

    let heartbeat = pull.heartbeat("client-1").await.unwrap();
    println!("heartbeat -> acknowledged={}", heartbeat.acknowledged);

    // FetchTask is a plain unary RPC: the call returns exactly once, with
    // exactly one TaskResponse in hand -- no partial results, no stream to
    // keep polling.
    let task = pull.fetch_task("client-1").await.unwrap();
    let weights = conflux_proto::decode_weights(&task.model_weights).unwrap();
    println!(
        "fetch_task -> task_id={:?}, round={}, weights={:?}",
        task.task_id, task.round, weights
    );

    // SubmitDelta streams chunks up to the server even in pull mode -- the
    // client sends a Vec<DeltaChunk> as an async stream, and the server
    // reads it message-by-message before replying with one SubmitAck.
    let chunks = vec![
        DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 2,
            data: conflux_proto::encode_weights(&[1.0, 2.0]),
            num_samples: 64,
        },
        DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 1,
            total_chunks: 2,
            data: conflux_proto::encode_weights(&[3.0]),
            num_samples: 64,
        },
    ];
    let ack = pull.submit_delta(chunks).await.unwrap();
    println!(
        "submit_delta -> accepted={}, {:?}",
        ack.accepted, ack.message
    );

    // Demonstrate the unary error path too: an RPC that comes back as a
    // tonic::Status surfaces as TransportError::Rpc, not a panic or a
    // silently-empty response.
    let err = pull.fetch_task("unregistered-client").await.unwrap_err();
    match err {
        TransportError::Rpc(status) => {
            println!(
                "fetch_task (unknown client) -> Rpc error, code={:?}",
                status.code()
            )
        }
        other => println!("fetch_task (unknown client) -> unexpected error: {other:?}"),
    }

    // --- Push mode: a genuine server-streaming RPC ---
    println!("\n== PushTransport (server streaming) ==");
    let mut push = PushTransport::connect(&addr).await.unwrap();
    push.register("client-2", "token").await.unwrap();

    // Unlike fetch_task, subscribe_tasks returns immediately with a
    // Streaming<TaskResponse> handle -- the three TaskResponses arrive one
    // at a time as separate messages on one long-lived connection, not as
    // one combined reply.
    let mut stream = push.subscribe_tasks("client-2").await.unwrap();
    let mut received = 0;
    while let Some(task) = stream.message().await.unwrap() {
        received += 1;
        println!(
            "subscribe_tasks -> received task_id={:?}, round={}",
            task.task_id, task.round
        );
    }
    println!("subscribe_tasks -> stream ended after {received} task(s)");
}
