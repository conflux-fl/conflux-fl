//! Client-side privacy transform tests (Phase 17) — local DP applied to
//! a client's own update before it leaves `conflux-node`.
//!
//! Every test here submits through a real local gRPC hop into a real
//! `NodeBridge` and inspects what a fake upstream `conflux-server`
//! actually received, rather than calling the transform directly. What
//! is being tested is the *pipeline stage*, not `GaussianClippingPrivacy`
//! itself — `conflux-privacy` already has its own tests for the
//! mechanism, and duplicating those here would prove nothing about
//! whether the node ever calls it.

use std::sync::{Arc, Mutex as StdMutex};

use conflux_net::{DispatchError, FlTransportService, PullTransport, RoundDispatcher, TaskStream};
use conflux_node::NodeBridge;
use conflux_privacy::GaussianClippingPrivacy;
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{
    DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse, decode_weights,
    encode_weights,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Records exactly what reached the network hop.
struct RecordingUpstream {
    received: Arc<StdMutex<Vec<DeltaChunk>>>,
}

#[async_trait::async_trait]
impl RoundDispatcher for RecordingUpstream {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        Ok(TaskResponse {
            task_id: "round-1".to_string(),
            round: 1,
            model_weights: Vec::new(),
            ..Default::default()
        })
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

fn chunk(index: u32, total: u32, weights: &[f32]) -> DeltaChunk {
    DeltaChunk {
        client_id: "py-client".to_string(),
        round: 1,
        chunk_index: index,
        total_chunks: total,
        data: encode_weights(weights),
        num_samples: 10,
        ..Default::default()
    }
}

/// Builds a bridge (optionally with local privacy), serves it on a real
/// loopback listener, and returns a client plus the upstream's record.
async fn harness(
    privacy: Option<(GaussianClippingPrivacy, Option<u64>)>,
) -> (PullTransport, Arc<StdMutex<Vec<DeltaChunk>>>) {
    let received = Arc::new(StdMutex::new(Vec::new()));
    let upstream_addr = spawn_grpc(RecordingUpstream {
        received: Arc::clone(&received),
    })
    .await;

    let transport = PullTransport::connect(upstream_addr).await.unwrap();
    let mut bridge = NodeBridge::new(transport, "node-1".to_string());
    if let Some((mechanism, seed)) = privacy {
        bridge = bridge.with_local_privacy(mechanism, seed);
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    let bridge = Arc::new(bridge);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let client = PullTransport::connect(format!("http://{local_addr}"))
        .await
        .unwrap();
    (client, received)
}

fn l2(weights: &[f32]) -> f32 {
    weights.iter().map(|w| w * w).sum::<f32>().sqrt()
}

#[tokio::test]
async fn with_the_transform_off_a_submission_arrives_byte_identical() {
    // The default path. If this ever fails, every pre-Phase-17
    // deployment's behavior has silently changed.
    let raw = vec![3.0f32, 4.0, 12.0];
    let (mut client, received) = harness(None).await;

    client.submit_delta(vec![chunk(0, 1, &raw)]).await.unwrap();

    let forwarded = received.lock().unwrap();
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded[0].data, encode_weights(&raw));
}

#[tokio::test]
async fn with_the_transform_on_the_update_is_clipped_and_noised_before_it_leaves() {
    // Raw L2 = 13, clip radius = 1, noise_multiplier chosen small enough
    // that the clipped norm still dominates — so the assertion below is
    // about clipping having happened, not about a lucky noise draw.
    let raw = vec![3.0f32, 4.0, 12.0];
    assert_eq!(l2(&raw), 13.0);

    let (mut client, received) = harness(Some((
        GaussianClippingPrivacy {
            clip_norm: 1.0,
            noise_multiplier: 0.01,
        },
        Some(7),
    )))
    .await;

    client.submit_delta(vec![chunk(0, 1, &raw)]).await.unwrap();

    let forwarded = received.lock().unwrap();
    let sent = decode_weights(&forwarded[0].data).unwrap();
    assert_ne!(sent, raw, "the raw update must not reach the network");
    // Clipped to radius 1, then perturbed by noise with sigma = 0.01 *
    // 1.0. A wide margin, deliberately: this asserts the update was
    // brought to roughly the clip radius, not that a specific noise
    // draw occurred.
    assert!(
        l2(&sent) < 1.5,
        "expected a clipped update near radius 1, got L2 {}",
        l2(&sent)
    );
}

#[tokio::test]
async fn clipping_applies_to_the_whole_update_not_to_each_chunk() {
    // The bug this exists to prevent: clipping each chunk separately to
    // radius 1 would let a 3-chunk update through at up to L2 sqrt(3),
    // so the actual privacy guarantee would depend on how the caller
    // happened to fragment its payload.
    let (mut client, received) = harness(Some((
        GaussianClippingPrivacy {
            clip_norm: 1.0,
            noise_multiplier: 0.0, // no noise: isolate the clipping bound
        },
        Some(1),
    )))
    .await;

    client
        .submit_delta(vec![
            chunk(0, 3, &[30.0, 40.0]),
            chunk(1, 3, &[50.0, 60.0]),
            chunk(2, 3, &[70.0, 80.0]),
        ])
        .await
        .unwrap();

    let forwarded = received.lock().unwrap();
    assert_eq!(forwarded.len(), 3, "chunk count must be preserved");
    // Reassemble the way the server does and check the whole-update norm.
    let mut sorted: Vec<_> = forwarded.clone();
    sorted.sort_by_key(|c| c.chunk_index);
    let mut bytes = Vec::new();
    for c in &sorted {
        bytes.extend_from_slice(&c.data);
    }
    let whole = decode_weights(&bytes).unwrap();
    assert_eq!(whole.len(), 6);
    assert!(
        (l2(&whole) - 1.0).abs() < 1e-4,
        "the whole update should be clipped to exactly the radius, got L2 {}",
        l2(&whole)
    );
    // And the wire shape is untouched.
    for (i, c) in sorted.iter().enumerate() {
        assert_eq!(c.chunk_index, i as u32);
        assert_eq!(c.total_chunks, 3);
        assert_eq!(c.data.len(), 8, "each chunk still carries its own 2 f32s");
    }
}

#[tokio::test]
async fn the_same_seed_produces_the_same_noise() {
    let raw = vec![0.1f32, 0.2, 0.3];
    let mechanism = || GaussianClippingPrivacy {
        clip_norm: 10.0, // above the raw norm: isolate the noise
        noise_multiplier: 1.0,
    };

    let (mut client_a, received_a) = harness(Some((mechanism(), Some(99)))).await;
    client_a
        .submit_delta(vec![chunk(0, 1, &raw)])
        .await
        .unwrap();

    let (mut client_b, received_b) = harness(Some((mechanism(), Some(99)))).await;
    client_b
        .submit_delta(vec![chunk(0, 1, &raw)])
        .await
        .unwrap();

    let a = received_a.lock().unwrap()[0].data.clone();
    let b = received_b.lock().unwrap()[0].data.clone();
    assert_eq!(a, b, "the same seed must produce the same noise");
    assert_ne!(a, encode_weights(&raw), "noise was actually added");
}

#[tokio::test]
async fn successive_submissions_do_not_repeat_the_same_noise() {
    // The failure a per-call re-seed would produce: identical noise
    // every round, which an observer can average away. The RNG is
    // seeded once and advanced instead.
    let raw = vec![0.1f32, 0.2, 0.3];
    let (mut client, received) = harness(Some((
        GaussianClippingPrivacy {
            clip_norm: 10.0,
            noise_multiplier: 1.0,
        },
        Some(42),
    )))
    .await;

    client.submit_delta(vec![chunk(0, 1, &raw)]).await.unwrap();
    client.submit_delta(vec![chunk(0, 1, &raw)]).await.unwrap();

    let forwarded = received.lock().unwrap();
    assert_eq!(forwarded.len(), 2);
    assert_ne!(
        forwarded[0].data, forwarded[1].data,
        "identical noise across rounds is noise an observer can subtract"
    );
}
