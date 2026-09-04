//! Runnable "try it" for the [crate-deep-dives article on
//! `conflux-node`](https://confluxfl.dev/crate-deep-dives/conflux-node/):
//! the two-hop bridge, with both connection modes and the client-side
//! privacy transform.
//!
//! Run with:
//!   cargo run --example local_hop -p conflux-node
//!
//! `conflux-node` is a proxy, not a driver. It opens one connection
//! upstream to `conflux-server`, then serves its own gRPC listener on
//! localhost and waits — the `ClientApp` is what drives the round by
//! calling `fetch_task` on that local hop. Both hops speak the same
//! `.proto`, which is what this example makes visible: the "Python
//! client" below is just another `conflux-net` transport.

use std::sync::{Arc, Mutex};

use conflux_net::{
    DispatchError, FlTransportService, PullTransport, PushTransport, RoundDispatcher, TaskStream,
};
use conflux_node::{ConnectionMode, NodeBridge};
use conflux_privacy::GaussianClippingPrivacy;
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{
    DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse, decode_weights,
    encode_weights,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::{ReceiverStream, TcpListenerStream};

/// Stands in for `conflux-server`, and records what actually reached it.
struct FakeServer {
    received: Arc<Mutex<Vec<Vec<f32>>>>,
}

#[async_trait::async_trait]
impl RoundDispatcher for FakeServer {
    async fn fetch_task(&self, _c: &str) -> Result<TaskResponse, DispatchError> {
        Ok(TaskResponse {
            task_id: "round-1".into(),
            round: 1,
            model_weights: encode_weights(&[0.5, 0.5, 0.5]),
            ..Default::default()
        })
    }
    async fn subscribe_tasks(&self, _c: &str) -> Result<TaskStream, DispatchError> {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tokio::spawn(async move {
            let _ = tx
                .send(Ok(TaskResponse {
                    task_id: "round-1".into(),
                    round: 1,
                    model_weights: encode_weights(&[0.5, 0.5, 0.5]),
                    ..Default::default()
                }))
                .await;
            std::future::pending::<()>().await;
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }
    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        for c in &chunks {
            self.received
                .lock()
                .unwrap()
                .push(decode_weights(&c.data).unwrap());
        }
        Ok(SubmitAck {
            accepted: true,
            message: "ok".into(),
        })
    }
    async fn register(
        &self,
        _c: &str,
        _t: &str,
        _f: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            message: String::new(),
        })
    }
    async fn heartbeat(&self, _c: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse { acknowledged: true })
    }
}

async fn spawn<D: RoundDispatcher>(d: D) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(Arc::new(d))))
            .serve_with_incoming(TcpListenerStream::new(l))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

async fn serve_bridge(bridge: Arc<NodeBridge>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
            .serve_with_incoming(TcpListenerStream::new(l))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

const TRAINED: [f32; 3] = [3.0, 4.0, 12.0]; // L2 norm exactly 13

#[tokio::main]
async fn main() {
    // --- pull mode ---------------------------------------------------
    let received = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(FakeServer {
        received: Arc::clone(&received),
    })
    .await;
    let bridge = Arc::new(NodeBridge::new(
        PullTransport::connect(upstream).await.unwrap(),
        "node-1".into(),
    ));
    println!(
        "bridge connection mode: {:?}",
        bridge.connection_mode().await
    );
    let local = serve_bridge(bridge).await;

    // The "Python ClientApp": an ordinary transport against the local hop.
    let mut client = PullTransport::connect(local).await.unwrap();
    let task = client.fetch_task("py-client").await.unwrap();
    println!(
        "\n=== pull mode ===\n  ClientApp asked the local hop and got round {} = {:?}",
        task.round,
        decode_weights(&task.model_weights).unwrap()
    );
    client.submit_delta(vec![chunk(&TRAINED)]).await.unwrap();
    println!("  it submitted {TRAINED:?}");
    println!("  the server received {:?}", received.lock().unwrap()[0]);
    println!("  -> forwarded unchanged: the node is a proxy, not a filter");

    // --- push mode ---------------------------------------------------
    let received = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(FakeServer {
        received: Arc::clone(&received),
    })
    .await;
    let bridge = Arc::new(NodeBridge::new_push(
        PushTransport::connect(upstream).await.unwrap(),
        "node-1".into(),
    ));
    assert_eq!(bridge.connection_mode().await, ConnectionMode::Push);
    let local = serve_bridge(bridge).await;
    let mut client = PushTransport::connect(local).await.unwrap();
    let mut stream = client.subscribe_tasks("py-client").await.unwrap();
    let pushed = stream.message().await.unwrap().unwrap();
    println!(
        "\n=== push mode ===\n  ClientApp subscribed and was pushed round {} without asking",
        pushed.round
    );
    println!("  the node relays one upstream subscription to the local hop,");
    println!("  reconnecting underneath if it drops — invisible from here");

    // --- client-side privacy ------------------------------------------
    let received = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(FakeServer {
        received: Arc::clone(&received),
    })
    .await;
    let bridge = Arc::new(
        NodeBridge::new(
            PullTransport::connect(upstream).await.unwrap(),
            "node-1".into(),
        )
        .with_local_privacy(
            GaussianClippingPrivacy {
                clip_norm: 1.0,
                noise_multiplier: 0.05,
            },
            Some(42),
        ),
    );
    let local = serve_bridge(bridge).await;
    let mut client = PullTransport::connect(local).await.unwrap();
    client.fetch_task("py-client").await.unwrap();
    client.submit_delta(vec![chunk(&TRAINED)]).await.unwrap();
    let sent = received.lock().unwrap()[0].clone();
    println!("\n=== with client-side privacy ===");
    println!(
        "  ClientApp submitted {TRAINED:?}  (L2 = {:.1})",
        l2(&TRAINED)
    );
    println!("  the server received  {:?}", round3(&sent));
    println!(
        "  (L2 = {:.3}, clipped to the configured radius of 1.0)",
        l2(&sent)
    );
    println!(
        "\n  The raw update never left the node. Clipping happens over the \
         whole reassembled update, not per chunk — clipping each chunk \
         separately would make the guarantee depend on how the caller \
         happened to fragment its payload."
    );
}

fn chunk(w: &[f32]) -> DeltaChunk {
    DeltaChunk {
        client_id: "py-client".into(),
        round: 1,
        chunk_index: 0,
        total_chunks: 1,
        data: encode_weights(w),
        num_samples: 10,
        ..Default::default()
    }
}
fn l2(w: &[f32]) -> f32 {
    w.iter().map(|x| x * x).sum::<f32>().sqrt()
}
fn round3(w: &[f32]) -> Vec<f32> {
    w.iter().map(|x| (x * 1000.0).round() / 1000.0).collect()
}
