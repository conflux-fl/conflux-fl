//! The same updates in a different arrival order must aggregate the same.
//!
//! A round's buffer collects submissions as they arrive, and arrival
//! order is whatever the network and the scheduler produced that round.
//! Most methods do not care — an average is an average. Selection-based
//! robust ones do: `bulyan` picks iteratively and breaks ties by
//! position, so the same set of updates in two orders was two results.
//!
//! `conflux-core` is already careful to be deterministic *given* an
//! ordering — `robust.rs` sorts the indices it selected, with a comment
//! saying why. Supplying that ordering is the server's job.
//!
//! This was found by moving the Rust baseline edge onto the real
//! pipeline: `bulyan` returned 0.9375, 0.8875 and 0.9250 on three
//! consecutive runs of identical inputs. Across processes it would have
//! looked like flakiness; in one process it was reproducible enough to
//! chase.

use std::sync::Arc;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::RoundDispatcher;
use conflux_proto::{DeltaChunk, encode_weights};
use conflux_registry::{ClientId, Registry};
use conflux_server::{AppState, run_round};
use conflux_store::Store;

const CLIENTS: usize = 7;

/// Distinct per client, so a method that picks a subset produces a
/// different answer depending on which it picked — which is what makes
/// this test able to fail.
fn update_for(i: usize) -> Vec<f32> {
    let base = i as f32;
    vec![base, base * 2.0, base * 3.0, base * 4.0]
}

fn config_with(aggregator: &str) -> conflux_config::ResolvedConfig {
    let merged = Overrides {
        aggregator: Some(aggregator.to_string()),
        // Wide enough to be inert, and no noise: this test is about
        // ordering, and a randomized transform would hide it.
        clip_norm: Some(1000.0),
        noise_multiplier: Some(0.0),
        round_timeout_secs: Some(5),
        // `bulyan` requires n >= 4f + 3; at seven clients that holds for
        // f = 1.
        robust_byzantine_fraction: Some(0.14),
        ..Default::default()
    };
    conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &merged)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap()
}

/// One round, with the clients submitting in `order`.
async fn aggregate_in_order(aggregator: &str, order: &[usize]) -> Vec<f32> {
    let state = Arc::new(AppState::new(
        config_with(aggregator),
        vec![0.0, 0.0, 0.0, 0.0],
    ));

    for i in 0..CLIENTS {
        state
            .registry
            .register(ClientId(format!("client-{i}")))
            .await
            .unwrap();
    }

    let round_state = Arc::clone(&state);
    let round_handle = tokio::spawn(async move { run_round(&round_state).await });

    for _ in 0..200 {
        if state
            .current_buffer
            .lock()
            .expect("mutex poisoned")
            .is_some()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // Sequentially, in exactly the order asked for — the point of the
    // test is that this order is the only thing differing between runs.
    for &i in order {
        state
            .submit_delta(vec![DeltaChunk {
                client_id: format!("client-{i}"),
                round: 1,
                chunk_index: 0,
                total_chunks: 1,
                data: encode_weights(&update_for(i)),
                num_samples: 5,
                ..Default::default()
            }])
            .await
            .unwrap();
    }

    round_handle.await.unwrap().unwrap();
    state.store.load_latest_weights().await.unwrap()
}

/// Every method in the catalog that this test can reach, forwards versus
/// backwards.
///
/// `bulyan` is the one that failed before the fix; the rest are here so
/// that a future method which *is* order-sensitive cannot be added
/// without this failing.
#[tokio::test(flavor = "multi_thread")]
async fn arrival_order_does_not_change_what_a_round_aggregates() {
    let forwards: Vec<usize> = (0..CLIENTS).collect();
    let backwards: Vec<usize> = (0..CLIENTS).rev().collect();
    // Neither sorted nor reversed, so a fix that happened to work for
    // reversal alone would not pass.
    let shuffled: Vec<usize> = vec![3, 0, 6, 1, 5, 2, 4];

    for aggregator in [
        "fedavg",
        "krum",
        "multi_krum",
        "trimmed_mean",
        "median",
        "bulyan",
    ] {
        let a = aggregate_in_order(aggregator, &forwards).await;
        let b = aggregate_in_order(aggregator, &backwards).await;
        let c = aggregate_in_order(aggregator, &shuffled).await;

        assert_eq!(
            a, b,
            "{aggregator}: reversing arrival order changed the aggregate\n  \
             forwards  {a:?}\n  backwards {b:?}"
        );
        assert_eq!(
            a, c,
            "{aggregator}: shuffling arrival order changed the aggregate\n  \
             forwards {a:?}\n  shuffled {c:?}"
        );
    }
}
