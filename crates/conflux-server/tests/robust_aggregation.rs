//! Proves each `robust` family member resolves through `conflux-config`'s
//! strategy registry and completes a real round end-to-end — the same
//! shape as `strategy_registry.rs`'s
//! `explicit_aggregator_and_selector_overrides_resolve_through_the_registry_end_to_end`.

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

/// Three identical clients, every `robust` aggregator name.
///
/// Three rather than one, and that is the whole subtlety. A single
/// submission used to work because each method's small-batch clamp
/// degrades to "return it unchanged" — but a batch of one is below what
/// Krum, Multi-Krum and Bulyan's papers cover, and the server now
/// refuses to aggregate outside a citation rather than clamping into it.
/// Three is the smallest batch every method here accepts: at the default
/// Byzantine fraction none of them excludes anybody, so Krum's `2f + 3`
/// and Bulyan's `4f + 3` both come to three.
///
/// Identical submissions, so every method still has one shared assertion
/// shape: whatever it selects or trims, the answer is that value.
async fn run_round_with_three_clients(aggregator_name: &str) -> Vec<f32> {
    let config = config_with(Overrides {
        aggregator: Some(aggregator_name.to_string()),
        ..Default::default()
    });
    let state = Arc::new(AppState::new(config, vec![1.0, 2.0]));

    for i in 1..=3 {
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

    for i in 1..=3 {
        state
            .submit_delta(vec![DeltaChunk {
                client_id: format!("client-{i}"),
                round: 1,
                chunk_index: 0,
                total_chunks: 1,
                data: encode_weights(&[10.0, 20.0]),
                num_samples: 5,
                ..Default::default()
            }])
            .await
            .unwrap();
    }

    let summary = round_handle.await.unwrap().unwrap();
    assert_eq!(summary.num_passed, 3);

    state.store.load_latest_weights().await.unwrap()
}

#[tokio::test]
async fn krum_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_round_with_three_clients("krum").await, vec![10.0, 20.0]);
}

#[tokio::test]
async fn multi_krum_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("multi_krum").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn trimmed_mean_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("trimmed_mean").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn median_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("median").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn faba_resolves_through_the_registry_end_to_end() {
    assert_eq!(run_round_with_three_clients("faba").await, vec![10.0, 20.0]);
}

#[tokio::test]
async fn bulyan_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("bulyan").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn geometric_median_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("geometric_median").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn median_of_means_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("median_of_means").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn divide_and_conquer_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("divide_and_conquer").await,
        vec![10.0, 20.0]
    );
}

#[tokio::test]
async fn foolsgold_resolves_through_the_registry_end_to_end() {
    assert_eq!(
        run_round_with_three_clients("foolsgold").await,
        vec![10.0, 20.0]
    );
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

/// A completed round lands in the history the admin API serves.
///
/// Driven through `run_round` rather than a running server on purpose:
/// recording lives there precisely so the path a test exercises is the
/// path a deployment takes, not a shorter one beside it.
#[tokio::test]
async fn a_completed_round_is_recorded_in_the_history() {
    let config = config_with(Overrides {
        aggregator: Some("fedavg".to_string()),
        round_history_len: Some(4),
        ..Default::default()
    });
    let state = Arc::new(AppState::new(config, vec![1.0, 2.0]));
    assert!(state.round_history.is_empty());

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
    round_handle.await.unwrap().unwrap();

    let recent = state.round_history.recent(10);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].round, 1);
    assert_eq!(recent[0].submitted, 1);
    assert_eq!(recent[0].passed, 1);
    // `fedavg` states no batch minimum, so there is no claim to report —
    // and `None` must not be flattened into `false` on the way out.
    assert_eq!(recent[0].cited_requirement_satisfied, None);
}
