//! A refused submission is two different situations, and the client has
//! to tell them apart.
//!
//! The server's round loop backs off and retries the **same round
//! number** after a failed attempt — a round it opened with nobody
//! registered, say, which closes on a quorum of zero the instant it
//! opens. So from a client's side a round can close and then reopen under
//! the same number.
//!
//! A client that treats every refusal as "this round is over for me" then
//! deadlocks against that: it waits for a round number greater than the
//! one it wrote off, and the server waits for a submission to the round
//! it just reopened. Neither is wrong on its own terms, and the run hangs
//! until something times out.
//!
//! The rule these tests pin down: after a refusal, ask what round the
//! server is on. Still this one — resubmit the same bytes. Moved on — the
//! work is spent, go and train the new round.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use conflux_client::{ClientApp, RunConfig, TrainResult};
use conflux_net::{DispatchError, FlTransportService, RoundDispatcher, TaskStream};
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{
    DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse, encode_weights,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// Stands in for `conflux-node`'s local hop, refusing the first
/// submission and then behaving.
struct RefusesOnce {
    round: AtomicU64,
    submits: AtomicUsize,
    /// Whether the refusal also advances the round — the difference
    /// between "closed and reopened" and "closed and moved on".
    advance_on_refusal: bool,
}

#[async_trait::async_trait]
impl RoundDispatcher for RefusesOnce {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        let round = self.round.load(Ordering::SeqCst);
        Ok(TaskResponse {
            task_id: format!("round-{round}"),
            round,
            model_weights: encode_weights(&[1.0, 2.0, 3.0]),
            control_variate: None,
        })
    }
    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        Ok(Box::pin(tokio_stream::empty()))
    }
    async fn submit_delta(&self, _chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        if self.submits.fetch_add(1, Ordering::SeqCst) == 0 {
            if self.advance_on_refusal {
                self.round.fetch_add(1, Ordering::SeqCst);
            }
            return Err(DispatchError::RoundClosed);
        }
        Ok(SubmitAck {
            accepted: true,
            message: "ok".to_string(),
        })
    }
    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            ..Default::default()
        })
    }
    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse::default())
    }
}

/// Counts training calls and records the rounds it was told ended.
struct Recorder {
    trained: Arc<AtomicUsize>,
    ended: Arc<std::sync::Mutex<Vec<(u64, bool)>>>,
}

impl ClientApp for Recorder {
    fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
        self.trained.fetch_add(1, Ordering::SeqCst);
        TrainResult::new(weights.to_vec(), 10)
    }
    fn on_round_end(&mut self, round: u64, accepted: bool) {
        self.ended
            .lock()
            .expect("mutex poisoned")
            .push((round, accepted));
    }
}

async fn serve(node: Arc<RefusesOnce>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(node)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

fn config(address: String) -> RunConfig {
    RunConfig {
        address,
        client_id: "c0".to_string(),
        rounds: 1,
        poll_interval: std::time::Duration::from_millis(5),
        ..RunConfig::default()
    }
}

#[tokio::test]
async fn a_refusal_on_a_round_that_has_not_moved_resubmits_the_same_work() {
    let node = Arc::new(RefusesOnce {
        round: AtomicU64::new(1),
        submits: AtomicUsize::new(0),
        advance_on_refusal: false,
    });
    let address = serve(Arc::clone(&node)).await;

    let trained = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut app = Recorder {
        trained: Arc::clone(&trained),
        ended: Arc::clone(&ended),
    };

    // Under a deadline, because the behaviour this pins down fails by
    // hanging: a client waiting for a round number it has written off
    // never stops waiting. A regression should fail the suite, not stall
    // it.
    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        conflux_client::run(&mut app, config(address)),
    )
    .await
    .expect("the client should not be waiting on a round it refuses to revisit")
    .unwrap();

    assert_eq!(completed, 1, "the round should have completed on the retry");
    assert_eq!(
        node.submits.load(Ordering::SeqCst),
        2,
        "the refused submission should have been offered again"
    );
    assert_eq!(
        trained.load(Ordering::SeqCst),
        1,
        "the same bytes should be resubmitted, not retrained — training again would \
         change the update the server is waiting for"
    );
    assert_eq!(*ended.lock().unwrap(), vec![(1, true)]);
}

/// The other half, and it passes before the fix as well as after — which
/// is the point of having it. "Do not write a round off too early" is one
/// sentence away from "never write one off", and that would resubmit into
/// a round the server has genuinely finished, forever. This is the guard
/// against overcorrecting.
#[tokio::test]
async fn a_refusal_on_a_round_that_has_moved_on_gives_up_and_trains_the_next() {
    let node = Arc::new(RefusesOnce {
        round: AtomicU64::new(1),
        submits: AtomicUsize::new(0),
        advance_on_refusal: true,
    });
    let address = serve(Arc::clone(&node)).await;

    let trained = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut app = Recorder {
        trained: Arc::clone(&trained),
        ended: Arc::clone(&ended),
    };

    // Under a deadline, because the behaviour this pins down fails by
    // hanging: a client waiting for a round number it has written off
    // never stops waiting. A regression should fail the suite, not stall
    // it.
    let completed = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        conflux_client::run(&mut app, config(address)),
    )
    .await
    .expect("the client should not be waiting on a round it refuses to revisit")
    .unwrap();

    assert_eq!(completed, 1, "round 2 should have completed");
    assert_eq!(
        trained.load(Ordering::SeqCst),
        2,
        "round 1's work is spent once the server has moved on; round 2 needs its own"
    );
    assert_eq!(
        *ended.lock().unwrap(),
        vec![(1, false), (2, true)],
        "the abandoned round should still be reported as ended, and not as accepted"
    );
}
