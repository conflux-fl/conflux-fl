//! Edge cases in this crate's own error-handling surface: what a client
//! transport does when there's nothing to connect to, and what a server
//! implementation's [`DispatchError`] variants and mid-stream failures
//! actually turn into on the wire. These are genuinely `conflux-net`'s own
//! logic (the `From<DispatchError> for Status` mapping in `dispatcher.rs`,
//! and the boxed `TaskStream` plumbing in `service.rs`) rather than
//! anything `conflux-server` adds on top.

use std::sync::Arc;

use conflux_net::{
    DispatchError, FlTransportService, PullTransport, RoundDispatcher, TaskStream, TransportError,
};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// A dispatcher whose `submit_delta` always reports the round as already
/// closed, and whose `subscribe_tasks` stream fails partway through.
struct FlakyDispatcher;

#[async_trait::async_trait]
impl RoundDispatcher for FlakyDispatcher {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        unreachable!("not exercised by these tests")
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        // One good message, then the stream itself yields an error — this
        // is different from the stream simply ending: a real dispatcher
        // might hit this if, say, its underlying broadcast channel closes
        // unexpectedly partway through a round.
        let items: Vec<Result<TaskResponse, tonic::Status>> = vec![
            Ok(TaskResponse {
                task_id: "task-1".to_string(),
                round: 1,
                model_weights: vec![],
            }),
            Err(tonic::Status::unavailable(
                "upstream task source disappeared",
            )),
        ];
        Ok(Box::pin(tokio_stream::iter(items)))
    }

    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        // Simulates conflux-buffer reporting its round already flushed —
        // the caller is expected to re-fetch and resubmit, not treat this
        // as a permanent failure.
        Err(DispatchError::RoundClosed)
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

async fn spawn_flaky_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = FlTransportService::new(Arc::new(FlakyDispatcher));
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

/// `PullTransport::connect` against an address nothing is listening on must
/// fail cleanly with `TransportError::Connect`, not hang or panic — this is
/// the ordinary "server is down" case every real client hits sooner or
/// later.
#[tokio::test]
async fn connect_to_a_closed_port_fails_with_a_connect_error() {
    // Bind and immediately drop a listener to get a port that's very likely
    // closed again by the time we try to connect to it.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let result = PullTransport::connect(format!("http://{addr}")).await;
    match result {
        Err(TransportError::Connect(_)) => {}
        Err(other) => panic!("expected TransportError::Connect, got {other:?}"),
        Ok(_) => panic!("connecting to a closed port must not succeed"),
    }
}

/// `DispatchError::RoundClosed` must reach the client as a distinguishable
/// `FailedPrecondition` status — not collapsed into the generic `Other` ->
/// `Status::internal` path — so a client can tell "resubmit against the
/// current round" apart from "something is actually broken."
#[tokio::test]
async fn round_closed_maps_to_failed_precondition_not_internal() {
    let addr = spawn_flaky_server().await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let err = transport
        .submit_delta(vec![DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: vec![],
            num_samples: 1,
            ..Default::default()
        }])
        .await
        .unwrap_err();

    match err {
        TransportError::Rpc(status) => {
            assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        }
        other => panic!("expected TransportError::Rpc, got {other:?}"),
    }
}

/// A `subscribe_tasks` stream that yields a real message and then an `Err`
/// (not just a clean end) must surface that error to the client at the
/// right point — the first message should still come through fine, and the
/// failure must not be silently swallowed or reported as a normal
/// end-of-stream.
#[tokio::test]
async fn a_stream_that_fails_midway_surfaces_the_error_after_the_good_message() {
    let addr = spawn_flaky_server().await;
    let mut transport = conflux_net::PushTransport::connect(addr).await.unwrap();

    let mut stream = transport.subscribe_tasks("client-1").await.unwrap();

    let first = stream.message().await.unwrap();
    assert!(first.is_some(), "the first, good message must still arrive");

    let second = stream.message().await;
    assert!(
        second.is_err(),
        "a stream error must surface as Err, not as a clean None end-of-stream"
    );
}
