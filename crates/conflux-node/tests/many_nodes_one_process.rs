//! Several nodes in one process, on ports nobody guessed.
//!
//! This is the property an in-process federation rests on. A node binds
//! its own local hop, so N nodes need N distinct ports; picking them by
//! convention — a base plus an index — is a guess that collides with
//! whatever else is on the machine, and checking a port is free before
//! binding it is a race, not a check.
//!
//! The answer is to bind `127.0.0.1:0`, let the OS assign, and read back
//! what it chose. Only whoever binds can learn that, which is why
//! [`conflux_node::run_on`] takes the listener rather than an address.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use conflux_net::{DispatchError, FlTransportService, RoundDispatcher, TaskStream};
use conflux_node::{ClientAppKind, ClientTls, ConnectionMode, NodeConfig, RuntimeMode, run_on};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Counts registrations, so the test can prove every node actually
/// reached the server rather than merely starting.
struct CountingUpstream {
    registrations: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl RoundDispatcher for CountingUpstream {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        Ok(TaskResponse::default())
    }
    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        Ok(Box::pin(tokio_stream::empty()))
    }
    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        Ok(SubmitAck::default())
    }
    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        self.registrations.fetch_add(1, Ordering::SeqCst);
        Ok(RegisterResponse {
            accepted: true,
            ..Default::default()
        })
    }
    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse::default())
    }
}

async fn spawn_upstream(registrations: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(Arc::new(
                CountingUpstream { registrations },
            ))))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

fn config_for(client_id: &str, server_addr: &str) -> NodeConfig {
    NodeConfig {
        mode: RuntimeMode::Research,
        allow_stub_client: true,
        client_app_kind: ClientAppKind::Stub,
        server_addr: server_addr.to_string(),
        client_id: client_id.to_string(),
        // Deliberately the same for every node, and deliberately
        // meaningless: `run_on` ignores it, which is the point. If it did
        // not, these nodes would all try to bind one port.
        local_addr: "127.0.0.1:47100".parse().unwrap(),
        connection_mode: ConnectionMode::Pull,
        local_privacy: None,
        privacy_seed: None,
        auth_token: "node-auth-token".to_string(),
        client_tls: ClientTls::Plaintext,
    }
}

#[tokio::test]
async fn five_nodes_share_a_process_and_no_two_share_a_port() {
    let registrations = Arc::new(AtomicUsize::new(0));
    let server_addr = spawn_upstream(Arc::clone(&registrations)).await;

    const N: usize = 5;
    let mut ports = Vec::new();
    for i in 0..N {
        // Bind first, then hand the listener over — the caller learns the
        // port, which is the whole reason `run_on` exists.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_ne!(port, 0, "the OS must have assigned a real port");
        ports.push(port);

        let config = config_for(&format!("node-{i}"), &server_addr);
        tokio::spawn(async move {
            // `pending()` — these nodes run until the test ends.
            let _ = run_on(listener, config, std::future::pending()).await;
        });
    }

    // Every port distinct: five nodes, five ports, none of them guessed.
    let mut unique = ports.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), N, "ports collided: {ports:?}");

    // And every node really registered upstream, so these are running
    // nodes rather than five processes that merely bound a socket.
    for _ in 0..50 {
        if registrations.load(Ordering::SeqCst) >= N {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    assert_eq!(
        registrations.load(Ordering::SeqCst),
        N,
        "every node should have registered with the one upstream"
    );
}
