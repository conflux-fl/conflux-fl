//! `NodeBridge` — the local hop's `RoundDispatcher` implementation.
//!
//! Bridges the local loopback hop (Python `ClientApp` ↔ `conflux-node`) to
//! the real network hop (`conflux-node` ↔ `conflux-server`): the same
//! `.proto`, reused for both, per ADR 0004.
//!
//! Both of the framework's connection modes are bridged the same way — a
//! call arriving on the local hop is forwarded upstream and its answer
//! relayed back — but they differ in shape. Pull mode forwards one
//! request and returns one response. Push mode forwards a *subscription*:
//! `conflux-node` holds one long-lived stream open against the server and
//! relays every task the server pushes down it to whatever local client
//! subscribed. That relay has to survive the upstream stream dropping,
//! which a single request/response call never has to think about — see
//! [`NodeBridge::subscribe_tasks`].

use std::time::Duration;

use conflux_net::{DispatchError, PullTransport, PushTransport, RoundDispatcher, TaskStream};
use conflux_privacy::GaussianClippingPrivacy;
use conflux_proto::{
    DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse, decode_weights,
    encode_weights,
};
use rand::SeedableRng;
use rand::rngs::StdRng;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::wrappers::ReceiverStream;
use tonic::Status;

/// Mirrors `conflux-config::ConnectionMode`, defined locally rather than
/// depending on that crate for one enum — the same deliberate scope
/// decision `startup_guard.rs` documents for `RuntimeMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMode {
    /// `cross_silo`'s default: few, trusted, always-reachable
    /// participants that can each hold an open connection.
    Push,
    /// The default everywhere else: many, intermittently-connected
    /// participants that check in on their own schedule.
    Pull,
}

impl ConnectionMode {
    /// The mode's canonical name, matching `conflux-config`'s spelling so
    /// a log line from the node and one from the server agree.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConnectionMode::Push => "push",
            ConnectionMode::Pull => "pull",
        }
    }
}

/// Which upstream transport this node opened at startup.
///
/// An enum rather than two `Option` fields because the two states are
/// genuinely exclusive: a node runs in exactly one connection mode for
/// the life of the process, and the compiler should be the thing
/// enforcing that rather than a runtime check on which field is `Some`.
/// It also means the mode-mismatch errors below are exhaustive by
/// construction — there's no "both `None`" case to forget about.
enum Upstream {
    Pull(PullTransport),
    Push(PushTransport),
}

/// `register`/`heartbeat` on the local hop are answered here without
/// touching the network — `conflux-node` already registered itself with
/// the real server at startup (spec §7); the local Python side isn't a
/// separate lifecycle entity the real server needs to track.
pub struct NodeBridge {
    upstream: Mutex<Upstream>,
    node_client_id: String,
    /// local DP applied to this client's own update before it
    /// leaves the node. `None` — the default — means the update is
    /// forwarded byte-for-byte as the `ClientApp` produced it, which is
    /// every pre-Phase-17 deployment's behavior exactly.
    ///
    /// This does not replace the server-side transform, which still runs
    /// afterwards. The two sit at different trust boundaries: this one
    /// keeps a raw update from ever being observable in the clear by the
    /// network or the server at all, which matters precisely when the
    /// server is not fully trusted.
    local_privacy: Option<LocalPrivacy>,
}

/// The mechanism plus the RNG that feeds it.
///
/// The RNG is owned and carried across calls rather than re-seeded per
/// submission. Re-seeding from a fixed seed each time would make every
/// round's noise identical — noise that repeats is noise an observer can
/// subtract, which defeats the mechanism entirely while still looking
/// like it works. Seeding once and advancing gives a reproducible
/// *sequence*, which is what research reproducibility actually needs.
struct LocalPrivacy {
    mechanism: GaussianClippingPrivacy,
    rng: Mutex<StdRng>,
}

impl NodeBridge {
    /// Pull-mode constructor. Keeps its original name (rather than
    /// becoming `new_pull` for symmetry with [`NodeBridge::new_push`])
    /// because pull mode's call sites predate push mode entirely and
    /// renaming them would churn working code to no benefit.
    pub fn new(upstream: PullTransport, node_client_id: String) -> Self {
        Self {
            upstream: Mutex::new(Upstream::Pull(upstream)),
            node_client_id,
            local_privacy: None,
        }
    }

    /// Push-mode constructor — `cross_silo`'s default posture.
    pub fn new_push(upstream: PushTransport, node_client_id: String) -> Self {
        Self {
            upstream: Mutex::new(Upstream::Push(upstream)),
            node_client_id,
            local_privacy: None,
        }
    }

    /// Turns on the client-side privacy transform.
    ///
    /// A consuming builder for the same reason `AppState::with_jwt_key`
    /// is one: both constructors above predate this, every existing call
    /// site passes exactly two arguments, and an optional stage should
    /// not become a required parameter everywhere.
    ///
    /// `seed` makes the noise sequence reproducible, matching
    /// `conflux-config`'s `seed_mode`/`seed_value` convention for
    /// research runs. Pass `None` for OS randomness.
    pub fn with_local_privacy(
        mut self,
        mechanism: GaussianClippingPrivacy,
        seed: Option<u64>,
    ) -> Self {
        let rng = match seed {
            Some(seed) => StdRng::seed_from_u64(seed),
            // Seeded from the OS entropy source via rand 0.10's
            // thread-local generator — the same way
            // `conflux-selector`'s `SelectionSeed::OsRandom` arm gets
            // its randomness.
            None => StdRng::from_rng(&mut rand::rng()),
        };
        self.local_privacy = Some(LocalPrivacy {
            mechanism,
            rng: Mutex::new(rng),
        });
        self
    }

    /// Applies the local transform across a whole submission.
    ///
    /// Chunks are reassembled first, deliberately. Clipping is defined
    /// over the L2 norm of the *entire* update — clipping each chunk
    /// separately to the same radius would bound each piece rather than
    /// the whole, letting a large update through in slices and producing
    /// a mechanism whose actual guarantee depends on how the caller
    /// happened to fragment its payload. Reassembly here mirrors what
    /// `conflux-server` already does on receipt (sort by `chunk_index`,
    /// concatenate).
    ///
    /// The original chunk boundaries are then restored byte-for-byte, so
    /// this changes the *contents* of a submission and nothing about its
    /// shape on the wire.
    async fn apply_local_privacy(
        &self,
        chunks: Vec<DeltaChunk>,
    ) -> Result<Vec<DeltaChunk>, DispatchError> {
        let Some(local) = &self.local_privacy else {
            return Ok(chunks);
        };

        let mut sorted = chunks;
        sorted.sort_by_key(|c| c.chunk_index);
        let lengths: Vec<usize> = sorted.iter().map(|c| c.data.len()).collect();
        let mut bytes = Vec::with_capacity(lengths.iter().sum());
        for chunk in &sorted {
            bytes.extend_from_slice(&chunk.data);
        }

        let mut weights = decode_weights(&bytes).map_err(|e| {
            DispatchError::Other(format!(
                "client-side privacy transform could not decode this submission's weights: {e}"
            ))
        })?;

        {
            let mut rng = local.rng.lock().await;
            local.mechanism.transform(&mut weights, &mut *rng);
        }

        // Re-split at exactly the original boundaries. `encode_weights`
        // is the same little-endian codec that produced the input, so
        // the total length is unchanged and every offset still lands.
        let transformed = encode_weights(&weights);
        let mut offset = 0;
        for (chunk, len) in sorted.iter_mut().zip(&lengths) {
            chunk.data = transformed[offset..offset + len].to_vec();
            offset += len;
        }

        Ok(sorted)
    }

    /// Which mode this bridge was built for. Lets a caller (today,
    /// `main.rs`'s startup logging) report the resolved mode without
    /// having to track it separately alongside the bridge.
    pub async fn connection_mode(&self) -> ConnectionMode {
        match &*self.upstream.lock().await {
            Upstream::Pull(_) => ConnectionMode::Pull,
            Upstream::Push(_) => ConnectionMode::Push,
        }
    }
}

const MAX_ATTEMPTS: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_millis(50);

/// How many pushed tasks may sit between the upstream subscription and
/// the local client before the relay stops reading from upstream.
///
/// Deliberately bounded, and small. A bounded channel propagates
/// backpressure: if the local `ClientApp` is busy training and isn't
/// reading, the relay stops pulling from the server rather than
/// accumulating an unbounded backlog of rounds nobody is working on. A
/// client that has fallen this far behind has a real problem, and
/// queueing more work for it would hide that rather than fix it.
const TASK_CHANNEL_CAPACITY: usize = 16;

/// Holds one upstream subscription open and relays what it yields into
/// `tx`, re-subscribing when it drops.
///
/// Runs as its own spawned task rather than inline in `subscribe_tasks`
/// because a subscription outlives the call that created it: the caller
/// gets a stream handle back immediately and reads from it for the rest
/// of the round, while this loop keeps the upstream side alive
/// underneath. The `mpsc` channel is the seam between the two — this
/// task owns the upstream stream and writes; the caller owns the
/// receiving end and reads.
///
/// Failure handling deliberately differs from `fetch_task`'s retry loop.
/// A failed `fetch_task` is one call to try again; a dropped
/// subscription is a *relationship* to re-establish, and it can fail in
/// a way a single call can't: the server may keep accepting
/// subscriptions while immediately closing each one. Counting only
/// subscribe errors would spin hot against that server forever, so what
/// actually resets the failure count here is a task being *delivered*,
/// not a subscription being *accepted*.
async fn relay_pushed_tasks(
    mut transport: PushTransport,
    client_id: String,
    tx: mpsc::Sender<Result<TaskResponse, Status>>,
) {
    let mut consecutive_failures: u32 = 0;
    let mut backoff = INITIAL_BACKOFF;

    loop {
        match transport.subscribe_tasks(&client_id).await {
            Ok(mut stream) => {
                let mut delivered_any = false;
                loop {
                    // `message()` is tonic's own stream reader: `Ok(Some)`
                    // is a task, `Ok(None)` a clean end-of-stream, `Err` a
                    // mid-stream failure. The last two are handled
                    // identically here — either way the subscription is
                    // gone and has to be re-established — but they're
                    // logged apart, since a server closing streams cleanly
                    // and a server erroring out are very different things
                    // to be looking at in production.
                    match stream.message().await {
                        Ok(Some(task)) => {
                            delivered_any = true;
                            consecutive_failures = 0;
                            backoff = INITIAL_BACKOFF;
                            if tx.send(Ok(task)).await.is_err() {
                                // The local client hung up. Nothing left
                                // to relay to, so stop — not an error.
                                tracing::debug!(
                                    %client_id,
                                    "local subscriber dropped; ending push relay"
                                );
                                return;
                            }
                        }
                        Ok(None) => {
                            tracing::info!(
                                %client_id,
                                delivered_any,
                                "upstream task stream closed; will re-subscribe"
                            );
                            break;
                        }
                        Err(status) => {
                            tracing::warn!(
                                %client_id,
                                error = %status,
                                "upstream task stream failed mid-stream; will re-subscribe"
                            );
                            break;
                        }
                    }
                }
                // A subscription that never delivered anything counts as
                // a failure even though subscribing itself succeeded —
                // this is what stops a server that accepts-then-closes
                // from being retried in a tight loop forever.
                if !delivered_any {
                    consecutive_failures += 1;
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!(
                    %client_id,
                    attempt = consecutive_failures,
                    error = %e,
                    ?backoff,
                    "subscribe_tasks attempt failed; retrying"
                );
            }
        }

        if consecutive_failures >= MAX_ATTEMPTS {
            // Surface this rather than stalling silently: a node that has
            // quietly stopped receiving tasks looks identical to an idle
            // one from the outside, which is exactly the failure this
            // reports out loud instead (ADR 0007).
            tracing::error!(
                %client_id,
                attempts = consecutive_failures,
                "push subscription failed {MAX_ATTEMPTS} times without receiving a task; giving up"
            );
            let _ = tx
                .send(Err(Status::unavailable(format!(
                    "push subscription to conflux-server failed {consecutive_failures} \
                     consecutive times without delivering a task"
                ))))
                .await;
            return;
        }

        // Slept outside the upstream lock and outside any borrow of the
        // stream, so a backing-off relay never blocks `submit_delta`.
        tokio::time::sleep(backoff).await;
        backoff *= 2;
    }
}

#[async_trait::async_trait]
impl RoundDispatcher for NodeBridge {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        let mut upstream = self.upstream.lock().await;
        let Upstream::Pull(transport) = &mut *upstream else {
            return Err(DispatchError::Other(
                "this conflux-node is running in push mode, where the server streams tasks \
                 rather than answering fetch_task — call subscribe_tasks instead, or start \
                 the node with CONFLUX_CONNECTION_MODE=pull"
                    .to_string(),
            ));
        };
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 1..=MAX_ATTEMPTS {
            match transport.fetch_task(&self.node_client_id).await {
                Ok(task) => return Ok(task),
                Err(e) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(attempt, error = %e, ?backoff, "fetch_task attempt failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(e) => return Err(DispatchError::Other(e.to_string())),
            }
        }
        unreachable!("loop always returns by the last attempt")
    }

    /// Opens one upstream subscription and hands back a stream of the
    /// tasks it yields.
    ///
    /// Returns as soon as the relay task is running, not once tasks
    /// start arriving — the caller gets a live stream handle and reads
    /// from it, exactly as it would from the server's own
    /// `subscribe_tasks`. Reconnection is invisible from here: a caller
    /// reading this stream sees a gap between tasks, not an error, when
    /// the upstream subscription drops and is re-established.
    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        // The lock is held only long enough to clone the transport, never
        // across the subscription's lifetime — see `PushTransport`'s own
        // note on why cloning is the right move for a long-lived stream
        // sharing a connection with ordinary calls.
        let transport = {
            let upstream = self.upstream.lock().await;
            let Upstream::Push(transport) = &*upstream else {
                return Err(DispatchError::Other(
                    "this conflux-node is running in pull mode, which has no server-pushed \
                     task stream — call fetch_task instead, or start the node with \
                     CONFLUX_CONNECTION_MODE=push"
                        .to_string(),
                ));
            };
            transport.clone()
        };

        let (tx, rx) = mpsc::channel(TASK_CHANNEL_CAPACITY);
        tokio::spawn(relay_pushed_tasks(
            transport,
            self.node_client_id.clone(),
            tx,
        ));
        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        // Transformed once, before the retry loop — a retry must resend
        // the same bytes, not re-noise them. Re-transforming per attempt
        // would draw fresh noise for each retry, so the server could
        // recover the raw update by averaging away the noise across
        // resends of what is supposed to be one submission.
        let chunks = self.apply_local_privacy(chunks).await?;

        let mut upstream = self.upstream.lock().await;
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 1..=MAX_ATTEMPTS {
            // Submitting works identically in both modes — only task
            // *acquisition* differs — so this matches on the upstream
            // purely to reach the right transport, not to behave
            // differently.
            let result = match &mut *upstream {
                Upstream::Pull(transport) => transport.submit_delta(chunks.clone()).await,
                Upstream::Push(transport) => transport.submit_delta(chunks.clone()).await,
            };
            match result {
                Ok(ack) => return Ok(ack),
                Err(e) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(attempt, error = %e, ?backoff, "submit_delta attempt failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(e) => return Err(DispatchError::Other(e.to_string())),
            }
        }
        unreachable!("loop always returns by the last attempt")
    }

    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            message: "conflux-node already registered with the real server".to_string(),
        })
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse { acknowledged: true })
    }
}
