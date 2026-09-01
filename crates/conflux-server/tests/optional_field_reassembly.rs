//! ADR 0012's new fields, through the server's real chunk reassembly.
//!
//! `submit_delta` is where `DeltaChunk`s become the `ClientDelta` every
//! aggregator consumes, so it is where the two new fields either survive
//! or silently vanish. The two behave differently on purpose and are
//! tested separately:
//!
//! - `local_steps` is a scalar a client repeats on every chunk, so it is
//!   read from whichever chunk arrives *first* — the same convention
//!   `num_samples` has always used, and for the same reason: depending on
//!   chunk 0 arriving first would be depending on network ordering.
//! - `control_variate` is a full vector chunked exactly like `data`, so
//!   it is concatenated in `chunk_index` order, not arrival order.
//!
//! A test that only ever submitted in-order chunks would pass with both
//! rules confused, so every case here submits out of order.

use std::sync::Arc;

use conflux_buffer::RoundBuffer;
use conflux_config::{Mode, Overrides, Topology};
use conflux_net::RoundDispatcher;
use conflux_proto::{DeltaChunk, decode_weights, encode_weights};
use conflux_server::AppState;

fn state_with_open_buffer() -> (Arc<AppState>, Arc<RoundBuffer>) {
    let config = conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        None,
        &Overrides::default(),
        &Overrides::default(),
    )
    .expect("config resolves");
    let state = Arc::new(AppState::new(config, vec![0.0; 4]));
    let buffer = Arc::new(RoundBuffer::new(1, 1));
    *state.current_buffer.lock().unwrap() = Some(Arc::clone(&buffer));
    (state, buffer)
}

/// Submits `chunks` and returns the single reassembled delta.
async fn reassemble(chunks: Vec<DeltaChunk>) -> conflux_proto::ClientDelta {
    let (state, buffer) = state_with_open_buffer();
    RoundDispatcher::submit_delta(state.as_ref(), chunks)
        .await
        .expect("submission accepted");
    let flushed = buffer.await_flush(std::time::Duration::from_secs(5)).await;
    flushed
        .deltas
        .into_iter()
        .next()
        .expect("exactly one delta was submitted")
}

fn chunk(index: u32, total: u32, data: Vec<u8>) -> DeltaChunk {
    DeltaChunk {
        client_id: "node-1".to_string(),
        round: 1,
        chunk_index: index,
        total_chunks: total,
        data,
        num_samples: 10,
        ..Default::default()
    }
}

#[tokio::test]
async fn a_submission_with_neither_field_reassembles_to_absent() {
    // The backward-compatible path, and by far the most common one: a
    // client that has never heard of ADR 0012 must produce a delta whose
    // new fields are absent — not zero, and not an empty vector, either
    // of which an aggregator could mistake for a real answer.
    let bytes = encode_weights(&[1.0, 2.0, 3.0, 4.0]);
    let mid = bytes.len() / 2;
    let delta = reassemble(vec![
        chunk(1, 2, bytes[mid..].to_vec()),
        chunk(0, 2, bytes[..mid].to_vec()),
    ])
    .await;

    assert_eq!(
        decode_weights(&delta.weights).unwrap(),
        vec![1.0, 2.0, 3.0, 4.0]
    );
    assert_eq!(delta.local_steps, None);
    assert_eq!(delta.control_variate, None);
}

#[tokio::test]
async fn local_steps_survives_when_chunks_arrive_out_of_order() {
    // Repeated on every chunk, so arrival order must not matter. Chunk 1
    // is submitted first.
    let bytes = encode_weights(&[1.0, 2.0, 3.0, 4.0]);
    let mid = bytes.len() / 2;
    let mut first = chunk(1, 2, bytes[mid..].to_vec());
    first.local_steps = Some(17);
    let mut second = chunk(0, 2, bytes[..mid].to_vec());
    second.local_steps = Some(17);

    let delta = reassemble(vec![first, second]).await;
    assert_eq!(delta.local_steps, Some(17));
}

#[tokio::test]
async fn the_control_variate_concatenates_in_chunk_index_order_not_arrival_order() {
    // The case that separates the two rules. If the control variate were
    // reassembled in *arrival* order the way `local_steps` is read, this
    // would come back as [3.0, 4.0, 1.0, 2.0] — a plausible-looking
    // vector of the right length, silently wrong.
    let variate = [1.0_f32, 2.0, 3.0, 4.0];
    let cv = encode_weights(&variate);
    let data = encode_weights(&[9.0_f32, 9.0, 9.0, 9.0]);
    let mid = cv.len() / 2;

    let mut first = chunk(1, 2, data[mid..].to_vec());
    first.control_variate = Some(cv[mid..].to_vec());
    let mut second = chunk(0, 2, data[..mid].to_vec());
    second.control_variate = Some(cv[..mid].to_vec());

    // Submitted 1-then-0, deliberately.
    let delta = reassemble(vec![first, second]).await;

    let recovered = decode_weights(&delta.control_variate.expect("present")).unwrap();
    assert_eq!(
        recovered, variate,
        "the control variate must reassemble by chunk_index, like `data`"
    );
}

#[tokio::test]
async fn both_fields_travel_together_without_interfering() {
    let variate = [0.1_f32, 0.2, 0.3, 0.4];
    let cv = encode_weights(&variate);
    let weights = [5.0_f32, 6.0, 7.0, 8.0];
    let data = encode_weights(&weights);
    let mid = cv.len() / 2;

    let mut c0 = chunk(0, 2, data[..mid].to_vec());
    c0.local_steps = Some(3);
    c0.control_variate = Some(cv[..mid].to_vec());
    let mut c1 = chunk(1, 2, data[mid..].to_vec());
    c1.local_steps = Some(3);
    c1.control_variate = Some(cv[mid..].to_vec());

    let delta = reassemble(vec![c1, c0]).await;

    assert_eq!(decode_weights(&delta.weights).unwrap(), weights);
    assert_eq!(delta.local_steps, Some(3));
    assert_eq!(
        decode_weights(&delta.control_variate.unwrap()).unwrap(),
        variate
    );
}

#[tokio::test]
async fn local_loss_survives_reassembly_and_absent_stays_absent() {
    // q-FedAvg's `F_k(w_t)`, added after `local_steps` and
    // `control_variate`. Same scalar convention: repeated on every
    // chunk, read from whichever arrives first.
    //
    // The second half matters more than the first. protobuf reads an
    // unset `optional float` as `0.0`, so "this client is not running
    // q-FedAvg" and "this client reported a loss of exactly zero" are
    // indistinguishable to anything checking truthiness — and with
    // `q > 0`, a loss read as zero means *zero weight*, silently
    // excluding every client that has not been upgraded to report one.
    let bytes = encode_weights(&[1.0, 2.0, 3.0, 4.0]);
    let mid = bytes.len() / 2;

    let mut first = chunk(1, 2, bytes[mid..].to_vec());
    first.local_loss = Some(0.75);
    let mut second = chunk(0, 2, bytes[..mid].to_vec());
    second.local_loss = Some(0.75);

    let delta = reassemble(vec![first, second]).await;
    assert_eq!(delta.local_loss, Some(0.75));

    // And a client that reports nothing must arrive as `None`.
    let silent = reassemble(vec![
        chunk(0, 2, bytes[..mid].to_vec()),
        chunk(1, 2, bytes[mid..].to_vec()),
    ])
    .await;
    assert_eq!(silent.local_loss, None, "absent must not become Some(0.0)");
}

#[tokio::test]
async fn all_three_optional_fields_travel_together() {
    // The full ADR 0012 payload as a real client now sends it: FedNova's
    // step count, q-FedAvg's loss, and SCAFFOLD's variate, in one
    // submission, out of order.
    let variate = [0.1_f32, 0.2, 0.3, 0.4];
    let cv = encode_weights(&variate);
    let weights = [5.0_f32, 6.0, 7.0, 8.0];
    let data = encode_weights(&weights);
    let mid = cv.len() / 2;

    let mut c0 = chunk(0, 2, data[..mid].to_vec());
    c0.local_steps = Some(30);
    c0.local_loss = Some(2.31);
    c0.control_variate = Some(cv[..mid].to_vec());
    let mut c1 = chunk(1, 2, data[mid..].to_vec());
    c1.local_steps = Some(30);
    c1.local_loss = Some(2.31);
    c1.control_variate = Some(cv[mid..].to_vec());

    let delta = reassemble(vec![c1, c0]).await;

    assert_eq!(decode_weights(&delta.weights).unwrap(), weights);
    assert_eq!(delta.local_steps, Some(30));
    assert_eq!(delta.local_loss, Some(2.31));
    assert_eq!(
        decode_weights(&delta.control_variate.unwrap()).unwrap(),
        variate
    );
}

#[tokio::test]
async fn a_partially_populated_control_variate_reassembles_short_rather_than_being_padded() {
    // Documented behavior, asserted so it stays deliberate. A client that
    // sets `control_variate` on only some of its chunks is malformed, but
    // the server cannot say so: it is opaque to model architecture (ADR
    // 0004), so it has no basis for knowing what length is correct. It
    // therefore concatenates what it was given and hands the result on
    // — short — for the consuming aggregator to reject on a length check.
    //
    // The alternative, silently zero-padding to the weights' length,
    // would manufacture a control variate the client never sent.
    let variate = [1.0_f32, 2.0, 3.0, 4.0];
    let cv = encode_weights(&variate);
    let data = encode_weights(&[9.0_f32, 9.0, 9.0, 9.0]);
    let mid = cv.len() / 2;

    let mut c0 = chunk(0, 2, data[..mid].to_vec());
    c0.control_variate = Some(cv[..mid].to_vec());
    let c1 = chunk(1, 2, data[mid..].to_vec()); // no control variate

    let delta = reassemble(vec![c0, c1]).await;

    let recovered = decode_weights(&delta.control_variate.expect("present")).unwrap();
    assert_eq!(
        recovered.len(),
        2,
        "half the vector was sent, so half arrives — not a padded four"
    );
    assert_eq!(
        decode_weights(&delta.weights).unwrap().len(),
        4,
        "and it no longer matches the weights' length, which is what the \
         aggregator checks"
    );
}
