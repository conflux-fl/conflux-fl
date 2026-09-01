//! (and its later extensions, FABA/Bulyan/Geometric Median):
//! proves each `robust` family member resolves through `conflux-config`'s
//! strategy registry and completes a real round end-to-end — the same
//! shape as the `strategy_registry.rs::
//! explicit_aggregator_and_selector_overrides_resolve_through_the_registry_end_to_end`.

use std::sync::Arc;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::RoundDispatcher;
use conflux_proto::{DeltaChunk, encode_weights};
use conflux_registry::{ClientId, Registry};
use conflux_server::{AppState, run_round};
use conflux_store::Store;

fn config_with(overrides: Overrides) -> conflux_config::ResolvedConfig {
    let mut merged = overrides;
    merged.clip_norm.get_or_insert(1000.0);
    merged.noise_multiplier.get_or_insert(0.0);
    merged.round_timeout_secs.get_or_insert(5);
    conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &merged)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap()
}

/// One client, every `robust` aggregator name — with a single submission
/// each method's own small-batch clamp degrades to "return it unchanged"
/// (Krum/Multi-Krum: nothing to filter out; Trimmed Mean/Median: nothing
/// to trim/no other value to combine with), so this proves each name
/// constructs a real, working `Aggregator` through the registry with one
/// shared assertion shape, the same way the fedavg test does.
async fn run_single_client_round(aggregator_name: &str) -> Vec<f32> {
    let config = config_with(Overrides {
        aggregator: Some(aggregator_name.to_string()),
        ..Default::default()
    });
    let state = Arc::new(AppState::new(config, vec![1.0, 2.0]));

    state
        .registry
        .register(ClientId("client-1".to_string()))
        .await
        .unwrap();

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

    state
        .submit_delta(vec![DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[10.0, 20.0]),
            num_samples: 5,
            ..Default::default()
        }])
        .await
        .unwrap();

    let summary = round_handle.await.unwrap().unwrap();
    assert_eq!(summary.num_passed, 1);

    state.store.load_latest_weights().await.unwrap()
}

#[tokio::test]
async fn krum_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_single_client_round("krum").await, vec![10.0, 20.0]);
}

#[tokio::test]
async fn multi_krum_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_single_client_round("multi_krum").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn trimmed_mean_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_single_client_round("trimmed_mean").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn median_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_single_client_round("median").await, vec![10.0, 20.0]);
}

#[tokio::test]
async fn faba_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_single_client_round("faba").await, vec![10.0, 20.0]);
}

#[tokio::test]
async fn bulyan_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_single_client_round("bulyan").await, vec![10.0, 20.0]);
}

#[tokio::test]
async fn geometric_median_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_single_client_round("geometric_median").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn median_of_means_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_single_client_round("median_of_means").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn divide_and_conquer_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_single_client_round("divide_and_conquer").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn foolsgold_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_single_client_round("foolsgold").await, vec![10.0, 20.0]);
}

#[test]
fn robust_byzantine_fraction_override_is_honored_by_construction() {
    // Not a panic test like an unknown name — just proves the value
    // actually reaches `AppState` via the same config path every other
    // resolved parameter uses.
    let config = config_with(Overrides {
        aggregator: Some("krum".to_string()),
        robust_byzantine_fraction: Some(0.4),
        ..Default::default()
    });

    assert_eq!(config.robust_byzantine_fraction.value, 0.4);
    let _state = AppState::new(config, vec![0.0]); // must not panic
}
