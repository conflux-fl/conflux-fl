//! Runnable "try it" for `conflux-server`: one complete federated round,
//! end to end, in a single process.
//!
//! Run with:
//!   cargo run --example round_pipeline -p conflux-server
//!
//! A real `AppState`, a real gRPC server, real clients connecting over
//! the network, and `run_round` driving the actual pipeline — the same
//! code path a deployment uses. Nothing here is mocked; the only thing
//! that isn't realistic is that the clients are in the same process.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::{FlTransportService, PullTransport};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{ClientDelta, decode_weights, encode_weights};
use conflux_registry::{ClientId, Registry};
use conflux_server::{AppState, run_round};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Three honest clients, plus one submitting something wild.
const CLIENTS: [(&str, [f32; 3]); 4] = [
    ("client-1", [1.0, 2.0, 3.0]),
    ("client-2", [1.1, 2.1, 2.9]),
    ("client-3", [0.9, 1.9, 3.1]),
    ("client-4", [50.0, 50.0, 50.0]),
];

#[tokio::main]
async fn main() {
    for aggregator in ["fedavg", "krum"] {
        run_one_round(aggregator).await;
    }

    println!(
        "\n  fedavg averages everyone, so the attacker drags the result a\n\
         quarter of the way to [50, 50, 50]. krum selects the single most\n\
         representative update and ignores the rest, so it lands on the\n\
         honest cluster."
    );
    println!(
        "\nSame clients, same submissions, one config value different.\n\
         `conflux-server` doesn't know what \"krum\" is — it asks\n\
         `conflux-core` to build whatever `config.aggregator.value`\n\
         names, which is why adding a method never touches this crate\n\
         (ADR 0002)."
    );
}

async fn run_one_round(aggregator: &str) {
    println!("\n=== a round with aggregator = {aggregator} ===");

    // Resolve configuration the same way `main.rs` does. `quorum = 4`
    // so the round closes as soon as everyone has submitted rather than
    // waiting out its timeout.
    let config = conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        None,
        &Overrides::default(),
        &Overrides {
            aggregator: Some(aggregator.to_string()),
            quorum: Some(CLIENTS.len() as u32),
            // The reputation filter would exclude the outlier before any
            // aggregator saw it, which would hide the difference this
            // example exists to show.
            min_reputation_score: Some(-1.0),
            // So would the privacy transform. It runs by default —
            // `gaussian_clipping` with `clip_norm = 1.0` and
            // `noise_multiplier = 1.0` — which clips every update to
            // unit norm and then adds noise of the same scale, so the
            // checkpoint comes out looking random and the aggregators
            // become indistinguishable. Worth knowing that this is on by
            // default; turned off here so the aggregation step is
            // actually visible.
            clip_norm: Some(1e9),
            noise_multiplier: Some(0.0),
            ..Default::default()
        },
    )
    .expect("config resolution failed");

    let state = Arc::new(AppState::new(config, vec![0.0, 0.0, 0.0]));
    for (id, _) in CLIENTS {
        state
            .registry
            .register(ClientId(id.to_string()))
            .await
            .unwrap();
    }

    // A real gRPC server on a real port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = Arc::clone(&state);
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(serving)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    // `run_round` opens the round; the clients then fetch and submit.
    let round_state = Arc::clone(&state);
    let round = tokio::spawn(async move { run_round(&round_state).await });

    // Wait for the buffer to open before any client asks for a task.
    while state.current_buffer.lock().unwrap().is_none() {
        tokio::task::yield_now().await;
    }

    for (id, weights) in CLIENTS {
        let mut client = PullTransport::connect(format!("http://{addr}"))
            .await
            .unwrap();
        client.register(id, "example-token").await.unwrap();
        let task = client.fetch_task(id).await.unwrap();
        println!(
            "  {id} received round {} ({} weights), submitting {weights:?}",
            task.round,
            decode_weights(&task.model_weights).unwrap().len()
        );
        client
            .submit_delta(vec![
                ClientDelta {
                    client_id: id.to_string(),
                    round: task.round,
                    weights: encode_weights(&weights),
                    num_samples: 10,
                    ..Default::default()
                }
                .into_chunk(),
            ])
            .await
            .unwrap();
    }

    let summary = round.await.unwrap().expect("the round should complete");
    let checkpoint = conflux_store::Store::load_latest_weights(&*state.store)
        .await
        .unwrap();

    println!(
        "\n  round {} closed on {:?}: {} selected, {} submitted, {} reached aggregation",
        summary.round,
        summary.flush_reason,
        summary.num_selected,
        summary.num_submitted,
        summary.num_passed
    );
    println!("  checkpoint -> {checkpoint:?}");
    println!("  next round is {}", state.round.load(Ordering::SeqCst));
}

/// `submit_delta` takes chunks; this example's updates are small enough
/// to be one each.
trait AsChunk {
    fn into_chunk(self) -> conflux_proto::DeltaChunk;
}

impl AsChunk for ClientDelta {
    fn into_chunk(self) -> conflux_proto::DeltaChunk {
        conflux_proto::DeltaChunk {
            client_id: self.client_id,
            round: self.round,
            chunk_index: 0,
            total_chunks: 1,
            data: self.weights,
            num_samples: self.num_samples,
            ..Default::default()
        }
    }
}
