//! A real federation, start to finish, inside the test binary.
//!
//! Three nodes, three Rust clients, two rounds. Every hop is the real
//! one: the clients reach their nodes over the local gRPC hop, the nodes
//! reach the server over loopback gRPC, and the server runs the same
//! buffer, flush, privacy, reputation, aggregation and checkpoint
//! pipeline it runs across machines. What this proves is that the whole
//! pipeline runs with nobody having named a port.
//!
//! Deliberately the only test in this file: the server reads the process
//! environment, which is global to a test binary, and an integration test
//! file is its own binary.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use conflux_federation::{ClientApp, FederationConfig, TrainResult};

/// Adds its own constant to whatever it was given.
///
/// Enough to make each client's contribution distinguishable in the
/// aggregate without dragging a model into a transport test.
struct AddShard {
    shard: f32,
    trained: Arc<AtomicUsize>,
}

impl ClientApp for AddShard {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        self.trained.fetch_add(1, Ordering::SeqCst);
        let updated: Vec<f32> = weights.iter().map(|w| w + self.shard).collect();
        // The sample count is what `fedavg` weights by; equal counts make
        // the expected aggregate a plain mean.
        TrainResult::new(updated, 100)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn three_clients_two_rounds_and_nobody_named_a_port() {
    // The federation's components log through `tracing`; without a
    // subscriber a failure here says nothing about which of them stopped.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_test_writer()
        .try_init();

    const CLIENTS: usize = 3;
    const ROUNDS: usize = 2;

    let trained: Vec<Arc<AtomicUsize>> = (0..CLIENTS)
        .map(|_| Arc::new(AtomicUsize::new(0)))
        .collect();
    let counters = trained.clone();

    let config = FederationConfig {
        nodes: CLIENTS,
        rounds: ROUNDS,
        // Generous, because this is a correctness test on a machine of
        // unknown load, not a timing one. It still has to exist: a
        // stalled client would otherwise hang the suite rather than fail
        // it.
        timeout: Duration::from_secs(30),
        ..FederationConfig::default()
    };

    let summary = conflux_federation::run(config, move |i| AddShard {
        shard: (i + 1) as f32,
        trained: Arc::clone(&counters[i]),
    })
    .await
    .expect("the federation should complete");

    assert_eq!(summary.clients.len(), CLIENTS);
    for (i, outcome) in summary.clients.iter().enumerate() {
        assert_eq!(outcome.index, i);
        assert_eq!(
            outcome.client_id,
            format!("client-{i}"),
            "node and client must share one identity, or the server sees an update from a \
             participant it never registered"
        );
        assert_eq!(
            outcome.rounds_completed, ROUNDS,
            "client {i} did not complete every round"
        );
        assert!(
            trained[i].load(Ordering::SeqCst) >= ROUNDS,
            "client {i} was asked for {ROUNDS} rounds but trained {} time(s)",
            trained[i].load(Ordering::SeqCst)
        );
    }

    // Both ports were assigned by the OS, so neither can be a number
    // anybody wrote down.
    assert_ne!(summary.server_grpc_addr.port(), 0);
    assert_ne!(summary.server_http_addr.port(), 0);
    assert_ne!(
        summary.server_grpc_addr.port(),
        summary.server_http_addr.port()
    );
}
