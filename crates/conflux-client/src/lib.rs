//! A Rust-native `ClientApp` — training with no Python hop.
//!
//! The same contract `python/conflux_client/app.py` offers, expressed in
//! Rust, so the architecture is exercised with a client that is not
//! Python.
//!
//! It is deliberately **not** a replacement for the Python SDK.
//! Researchers want PyTorch and every end-to-end harness is PyTorch.
//! This is a second path, and the question it answers is narrower than
//! "should the client be Rust": it answers *can* it be, and what that
//! would cost.
//!
//! # Why this matters beyond taste
//!
//! Two questions a Python client leaves open, a Rust client does not
//! answer so much as dissolve:
//!
//! | Question | Python | Rust |
//! |---|---|---|
//! | how does the client learn the model architecture? | an import path, or a serialized model over a new proto field | **compiled in** — there is no handoff |
//! | how does a participant *get* the client? | pip / container / something that pushes it | **one static binary** |
//!
//! The second is the one most clearly outside this codebase's boundary.
//! For `crowdsource` and `edge` — participants who
//! are not pre-provisioned machines — shipping a binary is a
//! categorically smaller problem than provisioning a Python environment.
//!
//! # What this does not decide
//!
//! **Which ML framework.** Two bundled examples show the range. `logreg`
//! hand-rolls logistic regression, exactly as `e2e_numpy_logreg` does on
//! the Python side — full-batch gradient descent over a flat weight
//! vector, no framework at all — which tests the *architecture* (does
//! the loop close, does the wire format fit, can a client be a single
//! binary) without committing the default build to a pre-1.0
//! dependency. `burn_mlp` (opt-in, `--features burn`) trains a real MLP
//! with [Burn](https://github.com/tracel-ai/burn)'s autodiff against the
//! same trait and the same flat-weight contract, and drives the real
//! `conflux-core` aggregators with it. Slotting in any other framework
//! means implementing [`ClientApp`] against it and changing nothing
//! here.
//!
//! # Example
//!
//! ```no_run
//! use conflux_client::{ClientApp, RunConfig, TrainResult, run};
//!
//! struct Doubler;
//!
//! impl ClientApp for Doubler {
//!     fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
//!         TrainResult::new(weights.iter().map(|w| w * 2.0).collect(), 100)
//!     }
//! }
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! run(&mut Doubler, RunConfig::default()).await?;
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

use conflux_net::{PullTransport, TransportError};
use conflux_proto::{DeltaChunk, decode_weights, encode_weights};

/// One chunk per this many bytes of payload.
///
/// `conflux-net` bounds a whole stream at `max_update_bytes`, but gRPC's
/// own ceiling is per *message* — so a model past roughly a million
/// parameters must be split or the send fails outright. 1 MiB leaves
/// generous headroom under gRPC's 4 MiB default.
pub const DEFAULT_CHUNK_BYTES: usize = 1 << 20;

/// Why a client run stopped.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The local hop to `conflux-node` failed.
    #[error(transparent)]
    Transport(#[from] TransportError),
    /// A weight buffer could not be decoded.
    #[error("could not decode the dispatched weights: {0}")]
    Codec(#[from] conflux_proto::WeightsCodecError),
    /// `train` returned a different number of weights than it was given.
    ///
    /// Its own variant rather than a generic error because the failure it
    /// prevents is subtle: every client in a round must agree on the
    /// model's shape, and a client that quietly returns a different one
    /// gets its whole batch rejected server-side with an error naming
    /// *someone else*.
    #[error(
        "train() returned {got} weights for a {expected}-weight model — every client in a \
         round must agree on the model's shape"
    )]
    ShapeChanged {
        /// How many weights were dispatched.
        expected: usize,
        /// How many came back.
        got: usize,
    },
}

/// What one round of local training produced.
///
/// Mirrors the Python SDK's `TrainResult` field for field, deliberately:
/// the two clients speak the same protocol, so a divergence between them
/// would be a bug in one of them rather than a design choice.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainResult {
    /// The trained weights — the same length as the ones handed in.
    ///
    /// Return *weights*, not a delta. Conflux transmits full vectors and
    /// the server computes whatever difference a given aggregator needs.
    pub weights: Vec<f32>,
    /// How many local examples this client trained on. FedAvg weights by
    /// it, and takes it on trust.
    pub num_samples: u64,
    /// Local optimization steps taken. **FedNova** reads this; `None`
    /// means "not running FedNova", which is a different fact from zero.
    pub local_steps: Option<u32>,
    /// This client's loss at the round's *starting* weights, before
    /// training. **q-FedAvg** reads this. Note the incentive: q-FedAvg
    /// weights *up* whoever reports a high loss.
    pub local_loss: Option<f32>,
    /// A control variate, same length as `weights`. **SCAFFOLD** reads
    /// this.
    pub control_variate: Option<Vec<f32>>,
}

impl TrainResult {
    /// The common case: trained weights and a sample count.
    pub fn new(weights: Vec<f32>, num_samples: u64) -> Self {
        Self {
            weights,
            num_samples,
            local_steps: None,
            local_loss: None,
            control_variate: None,
        }
    }

    /// Reports the local step count, for FedNova.
    pub fn with_local_steps(mut self, steps: u32) -> Self {
        self.local_steps = Some(steps);
        self
    }

    /// Reports the pre-training loss, for q-FedAvg.
    pub fn with_local_loss(mut self, loss: f32) -> Self {
        self.local_loss = Some(loss);
        self
    }

    /// Reports a control variate, for SCAFFOLD.
    ///
    /// Its length must match `weights`; the server cannot check that for
    /// you, being opaque to model architecture.
    pub fn with_control_variate(mut self, variate: Vec<f32>) -> Self {
        self.control_variate = Some(variate);
        self
    }
}

/// True for the server's generic all-zero starting checkpoint.
///
/// `conflux-server` is opaque to model architecture, so the only
/// initialization it can offer a model it knows nothing about is zeros.
/// That is harmless for a model with no hidden layers and a **textbook
/// symmetry-breaking failure** for anything with ReLU hidden units:
/// every unit computes an identical zero output with an identical zero
/// gradient, so none ever differentiates from another and the network
/// cannot learn from that start, however long it trains.
///
/// A real client recognizes this and substitutes its own
/// architecture-aware initialization — deterministically, so every
/// client agrees on the same starting point.
pub fn is_placeholder_init(weights: &[f32]) -> bool {
    !weights.is_empty() && weights.iter().all(|w| *w == 0.0)
}

/// What a client run needs to know.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// **`conflux-node`'s local listener**, not the server's. That hop is
    /// plaintext and localhost-only by design: the node has
    /// already authenticated upstream on this client's behalf.
    pub address: String,
    /// This client's identity.
    pub client_id: String,
    /// How many rounds to complete before returning.
    pub rounds: usize,
    /// Ignored on the local hop — see `address`. Present so the same
    /// config can drive a client talking directly to a server.
    pub auth_token: String,
    /// Payload bytes per chunk.
    pub chunk_bytes: usize,
    /// How long to wait before re-asking for a task when the round has
    /// not advanced.
    pub poll_interval: std::time::Duration,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            address: "http://127.0.0.1:47100".to_string(),
            client_id: "rust-client-1".to_string(),
            rounds: 1,
            auth_token: "client-token".to_string(),
            chunk_bytes: DEFAULT_CHUNK_BYTES,
            poll_interval: std::time::Duration::from_millis(200),
        }
    }
}

/// Implement this and hand it to [`run`].
///
/// `&mut self` here, unlike `conflux-core`'s `Aggregator`: a client owns
/// its model exclusively and runs one round at a time, so there is no
/// shared-access problem to work around. The interior-mutability rule
/// is about the *server* side, where one aggregator serves every round
/// behind an `Arc`.
pub trait ClientApp {
    /// Train on local data, starting from `weights`.
    ///
    /// `weights` is flat and architecture-free — unpack it into whatever
    /// model this client holds. If [`is_placeholder_init`] is true, the
    /// server had no checkpoint yet and you should use your own
    /// initialization instead of these zeros.
    fn train(&mut self, weights: &[f32], round: u64) -> TrainResult;

    /// Called before [`Self::train`]. Override for logging or setup.
    fn on_round_start(&mut self, _round: u64) {}

    /// The server's global control variate `c`, when the configured
    /// aggregator maintains one.
    ///
    /// **SCAFFOLD only.** Called immediately before [`Self::train`] on
    /// rounds where the server sent one, so an implementation can hold
    /// it and apply the `(c − c_i)` correction during local training.
    /// Never called otherwise, which is every aggregator but `scaffold`
    /// — so this stays a no-op for clients that do not implement it.
    ///
    /// `c` has the same length as the round's weights.
    fn on_control_variate(&mut self, _c: &[f32]) {}

    /// Called after submission. `accepted` is the server's own answer —
    /// `false` usually means the round closed on quorum or timeout while
    /// this client was still training, which is ordinary.
    fn on_round_end(&mut self, _round: u64, _accepted: bool) {}
}

/// Splits one result into chunks.
///
/// `weights` and `control_variate` are split at the same offsets,
/// because the server concatenates each independently in `chunk_index`
/// order and expects them to correspond. The scalars repeat on every
/// chunk — the server reads them from whichever arrives first, so
/// repeating costs a few bytes and removes any dependence on ordering.
fn build_chunks(
    client_id: &str,
    round: u64,
    result: &TrainResult,
    chunk_bytes: usize,
) -> Vec<DeltaChunk> {
    let payload = encode_weights(&result.weights);
    let variate = result.control_variate.as_ref().map(|v| encode_weights(v));

    // Align to f32 boundaries so a chunk never cuts a float in half —
    // the server concatenates raw bytes and would not notice.
    let step = chunk_bytes.max(4) / 4 * 4;
    let total = payload.len().div_ceil(step).max(1);

    (0..total)
        .map(|index| {
            let lo = index * step;
            let hi = ((index + 1) * step).min(payload.len());
            DeltaChunk {
                client_id: client_id.to_string(),
                round,
                chunk_index: index as u32,
                total_chunks: total as u32,
                data: payload[lo.min(payload.len())..hi].to_vec(),
                num_samples: result.num_samples,
                local_steps: result.local_steps,
                local_loss: result.local_loss,
                control_variate: variate
                    .as_ref()
                    .map(|v| v[lo.min(v.len())..hi.min(v.len())].to_vec()),
            }
        })
        .collect()
}

/// Runs `app` for `config.rounds` rounds. Returns how many submissions
/// the node took; whether the server *accepted* each one is reported
/// through [`ClientApp::on_round_end`].
pub async fn run<A: ClientApp>(app: &mut A, config: RunConfig) -> Result<usize, ClientError> {
    let mut transport = PullTransport::connect(config.address.clone()).await?;
    transport
        .register(&config.client_id, &config.auth_token)
        .await?;
    tracing::info!(client_id = %config.client_id, address = %config.address, "registered");

    let mut last_round: Option<u64> = None;
    let mut completed = 0usize;

    while completed < config.rounds {
        // Wait for a round we have not already done. The node answers
        // immediately with whatever is current, so without this a fast
        // client submits the same round repeatedly.
        let task = loop {
            let task = transport.fetch_task(&config.client_id).await?;
            if Some(task.round) != last_round {
                break task;
            }
            tokio::time::sleep(config.poll_interval).await;
        };

        let weights = decode_weights(&task.model_weights)?;
        app.on_round_start(task.round);

        // SCAFFOLD's `c`, when the server's aggregator maintains one.
        // Delivered before `train` because the correction is applied
        // *during* local training, not after it.
        if let Some(raw) = task.control_variate.as_ref() {
            let c = decode_weights(raw)?;
            app.on_control_variate(&c);
        }
        let result = app.train(&weights, task.round);

        if result.weights.len() != weights.len() {
            return Err(ClientError::ShapeChanged {
                expected: weights.len(),
                got: result.weights.len(),
            });
        }

        let chunks = build_chunks(&config.client_id, task.round, &result, config.chunk_bytes);
        match transport.submit_delta(chunks).await {
            Ok(ack) => {
                last_round = Some(task.round);
                completed += 1;
                app.on_round_end(task.round, ack.accepted);
                tracing::info!(round = task.round, accepted = ack.accepted, "submitted");
            }
            Err(e) => {
                // A round closing on quorum or timeout mid-training is
                // ordinary, not a failure. Move on rather than retrying
                // into a round that is already over.
                tracing::warn!(round = task.round, error = %e, "submission rejected; continuing");
                last_round = Some(task.round);
                app.on_round_end(task.round, false);
            }
        }
    }

    tracing::info!(completed, "done");
    Ok(completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholder_detection_matches_the_python_sdk() {
        assert!(is_placeholder_init(&[0.0, 0.0, 0.0]));
        assert!(!is_placeholder_init(&[0.0, 0.1]));
        assert!(!is_placeholder_init(&[]));
    }

    #[test]
    fn chunking_splits_both_vectors_in_lockstep_and_repeats_scalars() {
        let result = TrainResult::new((0..10).map(|i| i as f32).collect(), 7)
            .with_local_steps(3)
            .with_local_loss(0.5)
            .with_control_variate((0..10).map(|i| i as f32 * 0.1).collect());

        let chunks = build_chunks("c1", 4, &result, 16);
        assert_eq!(chunks.len(), 3, "40 bytes at 16 bytes/chunk");

        // Scalars repeat, so the server never depends on chunk 0 first.
        assert!(
            chunks
                .iter()
                .all(|c| c.num_samples == 7 && c.local_steps == Some(3))
        );
        assert!(chunks.iter().all(|c| c.local_loss == Some(0.5)));

        // Both vectors reassemble, at the same offsets.
        let data: Vec<u8> = chunks.iter().flat_map(|c| c.data.clone()).collect();
        assert_eq!(decode_weights(&data).unwrap(), result.weights);
        let cv: Vec<u8> = chunks
            .iter()
            .flat_map(|c| c.control_variate.clone().unwrap())
            .collect();
        assert_eq!(
            decode_weights(&cv).unwrap(),
            result.control_variate.unwrap()
        );

        // No chunk may cut an f32 in half.
        assert!(chunks.iter().all(|c| c.data.len().is_multiple_of(4)));
        assert_eq!(
            chunks.iter().map(|c| c.chunk_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn absent_optional_fields_stay_absent() {
        // The property the whole `optional` design rests on: "not running
        // q-FedAvg" and "reported a loss of exactly zero" must not
        // collapse into the same thing. Rust's `Option` makes this
        // harder to get wrong than Python's, where an unset `optional
        // float` reads as `0.0`.
        let chunks = build_chunks("c", 1, &TrainResult::new(vec![1.0], 1), 1 << 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].local_loss, None);
        assert_eq!(chunks[0].local_steps, None);
        assert_eq!(chunks[0].control_variate, None);
    }

    #[test]
    fn a_single_weight_still_produces_one_chunk() {
        let chunks = build_chunks("c", 1, &TrainResult::new(vec![1.0], 1), 1 << 20);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].total_chunks, 1);
    }

    #[test]
    fn an_empty_weight_vector_does_not_produce_zero_chunks() {
        // A degenerate case, but `submit_delta` with zero chunks is
        // rejected by the dispatcher with an error naming nothing useful.
        // One empty chunk is at least attributable.
        let chunks = build_chunks("c", 1, &TrainResult::new(vec![], 1), 1 << 20);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].data.is_empty());
    }
}
