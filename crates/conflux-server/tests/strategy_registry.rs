//! Proves `config.aggregator.value`/`config.selector.value`
//! actually drive construction through `conflux-config`'s strategy
//! registry — not just that the builtin defaults happen to still work.

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

#[tokio::test]
async fn explicit_aggregator_and_selector_overrides_resolve_through_the_registry_end_to_end() {
    let config = config_with(Overrides {
        aggregator: Some("fedavg".to_string()),
        selector: Some("uniform_random".to_string()),
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

    // Wait for the round to open its buffer, then submit directly
    // through the dispatcher (no gRPC needed — this test is about
    // construction, not transport).
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

    // FedAvg of one update is that update's weights unchanged — proves
    // the boxed `Aggregator` the registry constructed is a real, working
    // `FedAvg`, not a stub.
    assert_eq!(
        state.store.load_latest_weights().await.unwrap(),
        vec![10.0, 20.0]
    );
}

#[test]
fn unknown_aggregator_override_panics_at_construction_not_silently_falls_back() {
    let config = config_with(Overrides {
        aggregator: Some("does_not_exist".to_string()),
        ..Default::default()
    });

    let result = std::panic::catch_unwind(|| AppState::new(config, vec![0.0]));
    assert!(
        result.is_err(),
        "an unregistered aggregator name must fail loudly at construction, \
         not silently resolve to some default"
    );
}

#[test]
fn unknown_selector_override_panics_at_construction_not_silently_falls_back() {
    let config = config_with(Overrides {
        selector: Some("does_not_exist".to_string()),
        ..Default::default()
    });

    let result = std::panic::catch_unwind(|| AppState::new(config, vec![0.0]));
    assert!(
        result.is_err(),
        "an unregistered selector name must fail loudly at construction, \
         not silently resolve to some default"
    );
}
