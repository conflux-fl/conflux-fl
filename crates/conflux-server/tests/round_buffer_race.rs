//! Real reproduction of the `RoundBuffer` lost-update race
//! (`docs/phases/phase-10a-roundbuffer-race.md`) at the `conflux-server`
//! level: `run_round` retrying past `AggregatorError::EmptyBatch` leaves
//! `AppState.current_buffer` pointing at an already-flushed buffer, and a
//! late submission against it must be explicitly rejected, not silently
//! swallowed into a buffer nobody reads again.

use std::sync::Arc;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::{DispatchError, RoundDispatcher};
use conflux_proto::{DeltaChunk, encode_weights};
use conflux_server::{AppState, ServerError, run_round};

fn deterministic_config(overrides: &Overrides) -> conflux_config::ResolvedConfig {
    let mut merged = overrides.clone();
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
async fn late_submission_against_an_already_flushed_round_is_rejected_not_lost() {
    let config = deterministic_config(&Overrides::default());
    let state = Arc::new(AppState::new(config, vec![1.0, 2.0]));

    // Zero active clients: `selected` is empty, `quorum` (no override)
    // defaults to `selected.len() == 0`, so the buffer closes immediately
    // with an empty batch, `aggregator.aggregate(&[])` fails with
    // `EmptyBatch`, and `run_round` returns early — before advancing
    // `state.round` or clearing `current_buffer` — exactly the retry
    // precondition the phase brief describes.
    let result = run_round(&state).await;
    assert!(matches!(
        result,
        Err(ServerError::Aggregator(
            conflux_core::AggregatorError::EmptyBatch
        ))
    ));

    // `current_buffer` still points at round 1's now-closed buffer. A
    // submission that arrives in this exact window — the real race
    // window a retried round leaves open — must be explicitly rejected.
    let result = state
        .submit_delta(vec![DeltaChunk {
            client_id: "late".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[3.0, 4.0]),
            num_samples: 1,
        }])
        .await;

    assert!(
        matches!(result, Err(DispatchError::RoundClosed)),
        "a submission racing an already-flushed round must error with \
         RoundClosed, not silently succeed into a buffer nobody reads again: {result:?}"
    );
}
