//! Real over-the-wire tests: a live tonic server on a bound TCP port,
//! driven by a real `PullTransport`/`PushTransport` client — not just
//! prost encode/decode.

use std::sync::Arc;

use conflux_net::{
    DispatchError, FlTransportService, PullTransport, PushTransport, RoundDispatcher, TaskStream,
    TransportError,
};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

struct TestDispatcher;

#[async_trait::async_trait]
impl RoundDispatcher for TestDispatcher {
    async fn fetch_task(&self, client_id: &str) -> Result<TaskResponse, DispatchError> {
        if client_id == "unknown" {
            return Err(DispatchError::UnknownClient(client_id.to_string()));
        }
        Ok(TaskResponse {
            task_id: "task-1".to_string(),
            round: 1,
            model_weights: vec![0, 0, 128, 63], // f32 1.0, little-endian,
            ..Default::default()
        })
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        let tasks = vec![
            TaskResponse {
                task_id: "task-1".to_string(),
                round: 1,
                model_weights: vec![],
                ..Default::default()
            },
            TaskResponse {
                task_id: "task-2".to_string(),
                round: 2,
                model_weights: vec![],
                ..Default::default()
            },
        ];
        Ok(Box::pin(tokio_stream::iter(tasks.into_iter().map(Ok))))
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        Ok(SubmitAck {
            accepted: true,
            message: format!("received {} chunks", chunks.len()),
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

async fn spawn_server() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let service = FlTransportService::new(Arc::new(TestDispatcher));
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(service))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn pull_transport_round_trips_every_rpc() {
    let addr = spawn_server().await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let register = transport.register("client-1", "token").await.unwrap();
    assert!(register.accepted);

    let heartbeat = transport.heartbeat("client-1").await.unwrap();
    assert!(heartbeat.acknowledged);

    let task = transport.fetch_task("client-1").await.unwrap();
    assert_eq!(task.task_id, "task-1");
    assert_eq!(task.round, 1);

    let ack = transport
        .submit_delta(vec![DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: vec![],
            num_samples: 10,
            ..Default::default()
        }])
        .await
        .unwrap();
    assert!(ack.accepted);
    assert_eq!(ack.message, "received 1 chunks");
}

#[tokio::test]
async fn push_transport_streams_multiple_tasks() {
    let addr = spawn_server().await;
    let mut transport = PushTransport::connect(addr).await.unwrap();

    let mut stream = transport.subscribe_tasks("client-1").await.unwrap();

    let first = stream.message().await.unwrap().unwrap();
    assert_eq!(first.task_id, "task-1");

    let second = stream.message().await.unwrap().unwrap();
    assert_eq!(second.task_id, "task-2");

    let end = stream.message().await.unwrap();
    assert!(end.is_none());
}

#[tokio::test]
async fn unknown_client_maps_to_not_found_status() {
    let addr = spawn_server().await;
    let mut transport = PullTransport::connect(addr).await.unwrap();

    let err = transport.fetch_task("unknown").await.unwrap_err();

    match err {
        TransportError::Rpc(status) => assert_eq!(status.code(), tonic::Code::NotFound),
        other => panic!("expected TransportError::Rpc, got {other:?}"),
    }
}
