//! Proves reputation filtering is genuinely opt-in (off by default lets
//! every aggregator behave exactly as its own paper defines it, with no
//! framework-imposed interference) and that a non-finite submission is
//! excluded rather than failing the whole round, regardless of the flag.

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
    merged.quorum.get_or_insert(5);
    conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &merged)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap()
}

async fn run_five_client_round(
    overrides: Overrides,
    client_updates: [(&str, &[f32], u64); 5],
) -> Vec<f32> {
    let config = config_with(overrides);
    let dim = client_updates[0].1.len();
    let state = Arc::new(AppState::new(config, vec![0.0; dim]));

    for (client_id, _, _) in &client_updates {
        state
            .registry
            .register(ClientId(client_id.to_string()))
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

    for (client_id, weights, num_samples) in &client_updates {
        state
            .submit_delta(vec![DeltaChunk {
                client_id: client_id.to_string(),
                round: 1,
                chunk_index: 0,
                total_chunks: 1,
                data: encode_weights(weights),
                num_samples: *num_samples,
                ..Default::default()
            }])
            .await
            .unwrap();
    }

    round_handle.await.unwrap().unwrap();

    state.store.load_latest_weights().await.unwrap()
}

/// The concrete regression this guards: `krum` with a reputation filter
/// in front of it collapses to the same accuracy as undefended `fedavg`
/// against a large-magnitude attacker, because the filter's batch-mean
/// reference is dragged by the attacker and every honest client gets
/// rejected first. With reputation off by default, Krum's own selection
/// logic runs on the real batch and defends correctly, with no special
/// flags needed.
#[tokio::test]
async fn krum_defends_against_an_outlier_with_reputation_at_its_default() {
    let result = run_five_client_round(
        Overrides {
            aggregator: Some("krum".to_string()),
            robust_byzantine_fraction: Some(0.3),
            ..Default::default()
        },
        [
            ("honest-1", &[1.0, 1.0], 1),
            ("honest-2", &[1.1, 0.9], 1),
            ("honest-3", &[0.9, 1.1], 1),
            ("honest-4", &[1.0, 1.0], 1),
            ("attacker", &[1000.0, -1000.0], 1),
        ],
    )
    .await;

    assert!(
        result[0] < 2.0 && result[1] < 2.0,
        "Krum should defend against the outlier by default now: {result:?}"
    );
}

/// Reputation filtering is still available, and still works, when a
/// deployer explicitly opts into it — it is a capability, just not the
/// default. Undefended `fedavg` + an attacker whose update points in the
/// *opposite* direction from the honest majority (rather than a
/// large-magnitude outlier — see the note below) + reputation
/// explicitly on: since 4 of 5 clients are honest, the batch mean stays
/// dominated by the honest direction, so the attacker's cosine
/// similarity to it is strongly negative and gets rejected.
///
/// Deliberately *not* a large-magnitude attack like the outlier test
/// above: a large-magnitude attacker dominates the raw batch mean
/// enough to also drag honest clients' cosine scores down — reputation's
/// own known weakness, which leaving it off avoids but which an explicit
/// opt-in still carries. This test picks an attack shape the
/// raw-mean-based filter can actually handle, to prove the *opt-in path
/// itself* works correctly.
#[tokio::test]
async fn reputation_explicitly_enabled_still_filters_by_cosine_score() {
    let result = run_five_client_round(
        Overrides {
            aggregator: Some("fedavg".to_string()),
            reputation_filter_enabled: Some(true),
            min_reputation_score: Some(0.3),
            ..Default::default()
        },
        [
            ("honest-1", &[1.0, 1.0], 1),
            ("honest-2", &[1.0, 1.0], 1),
            ("honest-3", &[1.0, 1.0], 1),
            ("honest-4", &[1.0, 1.0], 1),
            ("attacker", &[-2.0, -2.0], 1),
        ],
    )
    .await;

    assert_eq!(
        result,
        vec![1.0, 1.0],
        "reputation, explicitly enabled, should reject the opposite-direction attacker: {result:?}"
    );
}

/// A single non-finite submission must not poison the shared reputation
/// reference for *everyone*. It is excluded outright, regardless of the
/// reputation flag, and the round completes normally with the remaining
/// honest clients.
#[tokio::test]
async fn a_non_finite_submission_is_excluded_not_a_whole_round_failure() {
    let result = run_five_client_round(
        Overrides {
            aggregator: Some("fedavg".to_string()),
            ..Default::default()
        },
        [
            ("honest-1", &[1.0, 1.0], 1),
            ("honest-2", &[1.0, 1.0], 1),
            ("honest-3", &[1.0, 1.0], 1),
            ("honest-4", &[1.0, 1.0], 1),
            ("degenerate", &[f32::NAN, f32::NAN], 1),
        ],
    )
    .await;

    assert_eq!(
        result,
        vec![1.0, 1.0],
        "the four honest clients should aggregate normally, degenerate client excluded: {result:?}"
    );
}

/// The exclusion above shortens the decoded batch. Every client *after*
/// the excluded one must still be aggregated with its own weights — a
/// positional pairing of deltas to decoded weights would silently give
/// each of them the next client's update.
#[tokio::test]
async fn an_excluded_submission_does_not_shift_the_weights_of_the_clients_after_it() {
    let result = run_five_client_round(
        Overrides {
            aggregator: Some("fedavg".to_string()),
            ..Default::default()
        },
        [
            ("degenerate", &[f32::NAN, f32::NAN], 1),
            ("honest-1", &[1.0, 1.0], 1),
            ("honest-2", &[2.0, 2.0], 1),
            ("honest-3", &[3.0, 3.0], 1),
            ("honest-4", &[4.0, 4.0], 1),
        ],
    )
    .await;

    assert_eq!(
        result,
        vec![2.5, 2.5],
        "the mean of the four honest clients, each with its own weights: {result:?}"
    );
}
