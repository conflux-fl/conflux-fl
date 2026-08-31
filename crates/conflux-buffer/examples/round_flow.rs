//! Runnable "try it" for the crate-deep-dives article on `conflux-buffer`.
//!
//! Run with:
//!   cargo run --example round_flow -p conflux-buffer
//!
//! Walks through the two ways a round's buffer actually closes — quorum
//! met before the timeout, and the timeout firing first with a partial
//! batch — plus what happens to a submission that arrives after the
//! buffer has already flushed.

use std::time::Duration;

use conflux_buffer::{FlushReason, RoundBuffer};
use conflux_proto::ClientDelta;

fn delta(client_id: &str, round: u64, num_samples: u64) -> ClientDelta {
    ClientDelta {
        client_id: client_id.to_string(),
        round,
        weights: vec![],
        num_samples,
        ..Default::default()
    }
}

#[tokio::main]
async fn main() {
    // 1. Quorum reached before the timeout: three clients submit almost
    //    immediately, quorum is 3, so `await_flush` returns as soon as the
    //    third push lands rather than waiting anywhere near the timeout.
    let round_1 = RoundBuffer::new(1, 3);
    round_1.push(delta("client-a", 1, 100)).unwrap();
    round_1.push(delta("client-b", 1, 100)).unwrap();
    round_1.push(delta("client-c", 1, 100)).unwrap();

    let result = round_1.await_flush(Duration::from_secs(30)).await;
    println!(
        "round 1: closed on {:?} with {} of 3 clients (timeout was 30s)",
        result.reason,
        result.deltas.len()
    );
    assert_eq!(result.reason, FlushReason::Quorum);

    // A push after the buffer has already flushed is rejected explicitly
    // — it never silently lands in a batch nobody will read again.
    let late = round_1.push(delta("client-late", 1, 100));
    println!("round 1: late push after flush -> {late:?}");

    // 2. Timeout fires first: quorum is 5, but only 2 clients ever submit.
    //    `await_flush` waits out the (short, for this demo) timeout and
    //    returns whatever partial batch it collected, rather than hanging
    //    forever for clients that never show up.
    let round_2 = RoundBuffer::new(2, 5);
    round_2.push(delta("client-x", 2, 50)).unwrap();
    round_2.push(delta("client-y", 2, 50)).unwrap();

    let result = round_2.await_flush(Duration::from_millis(100)).await;
    println!(
        "round 2: closed on {:?} with {} of 5 clients (timeout was 100ms)",
        result.reason,
        result.deltas.len()
    );
    assert_eq!(result.reason, FlushReason::Timeout);

    // 3. A submission for the wrong round is rejected up front — the
    //    buffer only ever collects deltas for the round it was created for.
    let wrong_round = round_2.push(delta("client-z", 99, 50));
    println!("round 2: push tagged for round 99 -> {wrong_round:?}");
}
