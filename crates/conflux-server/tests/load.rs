//! Concurrency/load validation: many simulated clients
//! (`conflux-net::PullTransport`, the same real client every other
//! integration test in this session uses) against one real, running
//! `AppState` + gRPC server, across several rounds — not the single
//! client every other test used. See
//! `docs/phases/phase-7g-load-testing.md`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::{FlTransportService, PullTransport};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, decode_weights, encode_weights};
use conflux_registry::{ClientId, Registry};
use conflux_server::{AppState, run_round};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

const NUM_CLIENTS: usize = 30;
const NUM_ROUNDS: u64 = 3;

fn deterministic_config(overrides: &Overrides) -> conflux_config::ResolvedConfig {
    let mut merged = overrides.clone();
    merged.clip_norm.get_or_insert(1000.0);
    merged.noise_multiplier.get_or_insert(0.0);
    merged.round_timeout_secs.get_or_insert(10);
    conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &merged)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap()
}

async fn spawn_grpc(state: Arc<AppState>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

async fn wait_until_buffer_open(state: &AppState) {
    for _ in 0..200 {
        if state
            .current_buffer
            .lock()
            .expect("mutex poisoned")
            .is_some()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run_round never opened this round's buffer");
}

#[tokio::test]
async fn concurrent_clients_across_multiple_rounds() {
    let config = deterministic_config(&Overrides::default());
    let state = Arc::new(AppState::new(config, vec![0.0; 4]));

    for i in 0..NUM_CLIENTS {
        state
            .registry
            .register(ClientId(format!("client-{i}")))
            .await
            .unwrap();
    }

    let addr = spawn_grpc(Arc::clone(&state)).await;
    let mut round_buffer_race_observed = false;
    let overall_start = Instant::now();

    for round in 1..=NUM_ROUNDS {
        let round_start = Instant::now();

        let round_state = Arc::clone(&state);
        let round_handle = tokio::spawn(async move { run_round(&round_state).await });
        wait_until_buffer_open(&state).await;

        let mut client_handles = Vec::with_capacity(NUM_CLIENTS);
        for i in 0..NUM_CLIENTS {
            let addr = addr.clone();
            client_handles.push(tokio::spawn(async move {
                let client_id = format!("client-{i}");
                let mut transport = PullTransport::connect(addr).await?;
                let task = transport.fetch_task(&client_id).await?;
                let weights = decode_weights(&task.model_weights).unwrap();
                let trained: Vec<f32> = weights.iter().map(|w| w + 1.0).collect();
                transport
                    .submit_delta(vec![DeltaChunk {
                        client_id,
                        round: task.round,
                        chunk_index: 0,
                        total_chunks: 1,
                        data: encode_weights(&trained),
                        num_samples: 10,
                        ..Default::default()
                    }])
                    .await
            }));
        }

        let mut accepted = 0;
        for handle in client_handles {
            let ack = handle
                .await
                .expect("client task panicked")
                .expect("client RPC failed under concurrent load");
            if ack.accepted {
                accepted += 1;
            }
        }

        let summary = round_handle
            .await
            .expect("run_round task panicked")
            .expect("run_round failed under concurrent load");

        // The known Phase 6/7d RoundBuffer race would show up as
        // num_submitted/num_passed less than NUM_CLIENTS despite every
        // client's RPC reporting success above — record honestly rather
        // than hiding a soft failure behind an unconditional assert.
        if summary.num_submitted != NUM_CLIENTS || summary.num_passed != NUM_CLIENTS {
            round_buffer_race_observed = true;
        }
        assert_eq!(
            accepted, NUM_CLIENTS,
            "every client's submit_delta RPC must succeed"
        );
        assert_eq!(summary.round, round);

        println!(
            "[load] round {round} completed in {:?} ({NUM_CLIENTS} clients, \
             {}/{NUM_CLIENTS} submitted, {}/{NUM_CLIENTS} passed reputation)",
            round_start.elapsed(),
            summary.num_submitted,
            summary.num_passed,
        );
    }

    println!(
        "[load] all {NUM_ROUNDS} rounds x {NUM_CLIENTS} clients completed in {:?}",
        overall_start.elapsed()
    );
    println!("[load] RoundBuffer race observed: {round_buffer_race_observed}");

    assert!(
        !round_buffer_race_observed,
        "the known Phase 6/7d RoundBuffer race manifested under this load — see \
         docs/STATUS.md's tracked deviation, this needs escalating from \"documented\" \
         to \"actively causing test failures\""
    );
}
