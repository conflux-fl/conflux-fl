//! An update must be the length this round dispatched.
//!
//! `conflux-proto` already states the invariant — a `ClientDelta`'s
//! weights are "the same length as the TaskResponse this client trained
//! from" — and nothing enforced it.
//!
//! `conflux-core::decode_and_validate` compares every update against
//! *the first update in the batch*, which is the most it can do:
//! `Aggregator::aggregate` receives only `&[ClientDelta]` and cannot
//! know what the round dispatched. A consistency check has two blind
//! spots that a consistency check must have, and this file is one test
//! for each:
//!
//! 1. A federation where **every** client runs the wrong architecture
//!    agrees with itself, passes, and silently replaces the global model
//!    with one of a different dimension.
//! 2. When one client differs, the error names whichever disagrees with
//!    the first — which may be the only correct one.
//!
//! The server is the only place that knows the dispatched length.

use std::sync::Arc;

use conflux_config::{Mode, Overrides, Topology};
use conflux_net::RoundDispatcher;
use conflux_proto::{DeltaChunk, encode_weights};
use conflux_registry::{ClientId, Registry};
use conflux_server::{AppState, run_round};
use conflux_store::Store;

/// The dimension the server starts with, and therefore dispatches.
const DIM: usize = 4;

fn config() -> conflux_config::ResolvedConfig {
    let merged = Overrides {
        aggregator: Some("fedavg".to_string()),
        // Inert privacy: this test is about lengths, and a randomized
        // transform would only add noise to the assertions.
        clip_norm: Some(1000.0),
        noise_multiplier: Some(0.0),
        round_timeout_secs: Some(5),
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

/// Runs one round in which each client submits an update of the given
/// length, and reports whether the round succeeded and what the stored
/// model looks like afterwards.
///
/// `lengths` is indexed by client, and the client ids are `client-0..n`
/// so that sorted order matches the slice order — which is what lets the
/// "first in the batch" case below be set up deliberately.
async fn round_with(lengths: &[usize]) -> (bool, Vec<f32>) {
    let state = Arc::new(AppState::new(config(), vec![0.5; DIM]));

    for i in 0..lengths.len() {
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

    for (i, &len) in lengths.iter().enumerate() {
        // Distinct per client, so aggregating a different subset gives a
        // different answer — otherwise these assertions could not fail.
        let update = vec![i as f32 + 1.0; len];
        state
            .submit_delta(vec![DeltaChunk {
                client_id: format!("client-{i}"),
                round: 1,
                chunk_index: 0,
                total_chunks: 1,
                data: encode_weights(&update),
                num_samples: 5,
                ..Default::default()
            }])
            .await
            .unwrap();
    }

    let ok = round_handle.await.unwrap().is_ok();
    let stored = state.store.load_latest_weights().await.unwrap();
    (ok, stored)
}

/// Blind spot 1: a batch that agrees with itself and is wrong.
///
/// Every client trains a 9-parameter model against a 4-parameter round.
/// They agree with each other, so a check that compares updates against
/// one another sees nothing — and the round would checkpoint a global
/// model of a dimension the federation never dispatched.
#[tokio::test(flavor = "multi_thread")]
async fn a_federation_that_agrees_on_the_wrong_length_is_refused() {
    let (ok, stored) = round_with(&[9, 9, 9]).await;

    assert!(
        !ok,
        "a round in which every update was the wrong length should not succeed"
    );
    assert_eq!(
        stored.len(),
        DIM,
        "the stored model changed dimension: the round replaced a {DIM}-parameter \
         global with a {}-parameter one",
        stored.len()
    );
    assert_eq!(
        stored,
        vec![0.5; DIM],
        "the stored model was modified by a round that should have produced nothing"
    );
}

/// Blind spot 2, and the ordering that makes it bite: the *first* client
/// in the batch is the wrong one.
///
/// Sorted by client id, `client-0` leads. Comparing everything against
/// the first update would take 9 as the expected length and reject the
/// two correct clients instead of the one incorrect one.
#[tokio::test(flavor = "multi_thread")]
async fn the_wrong_client_is_excluded_even_when_it_sorts_first() {
    let (ok, stored) = round_with(&[9, DIM, DIM]).await;

    assert!(ok, "the correctly-sized clients should still make a round");
    assert_eq!(stored.len(), DIM);
    // client-1 and client-2 submitted 2.0 and 3.0 with equal sample
    // counts; client-0's 1.0 must not be in the mean.
    assert_eq!(
        stored,
        vec![2.5; DIM],
        "expected the mean of client-1 and client-2 only"
    );
}

/// The same batch with the wrong client last, so the fix is not passing
/// by accident of position.
#[tokio::test(flavor = "multi_thread")]
async fn the_wrong_client_is_excluded_when_it_sorts_last() {
    let (ok, stored) = round_with(&[DIM, DIM, 9]).await;

    assert!(ok);
    // client-0 and client-1 submitted 1.0 and 2.0.
    assert_eq!(stored, vec![1.5; DIM]);
}

/// The honest case still works, so the check is not simply rejecting
/// everything.
#[tokio::test(flavor = "multi_thread")]
async fn a_correctly_sized_batch_is_untouched() {
    let (ok, stored) = round_with(&[DIM, DIM, DIM]).await;

    assert!(ok);
    assert_eq!(stored, vec![2.0; DIM], "mean of 1.0, 2.0 and 3.0");
}
