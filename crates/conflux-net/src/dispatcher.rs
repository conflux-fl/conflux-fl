//! The seam between conflux-net's transport/wire mechanics and whatever
//! decides how to actually answer each RPC. `conflux-net` only knows how to
//! move bytes over gRPC; it has no opinion on client registries, buffering,
//! or aggregation. `conflux-server`'s `AppState` implements
//! [`RoundDispatcher`] for real, wiring in `conflux-registry` (client
//! lifecycle), `conflux-buffer` (round staging), and `conflux-store` (model
//! checkpoints); this crate's own tests implement a trivial in-memory
//! dispatcher instead, so the transport layer can be tested without any of
//! that machinery.

use std::pin::Pin;

use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio_stream::Stream;
use tonic::Status;

/// The stream type `subscribe_tasks` (push mode) returns — boxed because
/// each `RoundDispatcher` implementation will build it differently (a
/// broadcast channel, a per-client queue, ...); the trait doesn't need to
/// know which.
pub type TaskStream = Pin<Box<dyn Stream<Item = Result<TaskResponse, Status>> + Send + 'static>>;

/// Every error a `RoundDispatcher` implementation can return. Mapped to a
/// `tonic::Status` at the `FlTransportService` boundary — see the
/// `From<DispatchError> for Status` impl below — rather than leaked as a
/// raw string.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("client {0} is not registered")]
    /// No such client is registered.
    UnknownClient(String),
    /// Returned when node-auth enforcement (`require_node_auth`) is on and
    /// this registration's presented identity — an mTLS peer cert
    /// fingerprint if the connection used TLS, otherwise the request's
    /// shared token — either isn't on the allow-list at all, or doesn't
    /// match what `client_id` was allowed to present.
    #[error("client {0} is not on the node allow-list")]
    NotAllowed(String),
    /// The caller's credential itself didn't check out — an invalid,
    /// expired, or wrong-subject JWT. Kept distinct from
    /// [`DispatchError::NotAllowed`] because the two mean opposite
    /// things about the caller: `NotAllowed` says "we know who you are
    /// and you aren't invited," this says "we couldn't establish who you
    /// are at all." They also map to different gRPC statuses, which is
    /// what lets a client tell "refresh your token" apart from "ask an
    /// operator to add you."
    #[error("authentication failed: {0}")]
    Unauthenticated(String),
    /// The round this submission targeted already flushed its buffer
    /// (`conflux-buffer`'s buffer closes once it hits quorum or times out)
    /// — the caller should re-`fetch_task` and resubmit against whatever
    /// round is current now, not treat this as a permanent failure. Kept
    /// distinct from `Other` so a client can actually tell the two apart
    /// instead of both showing up as `Status::internal`.
    #[error("this round already closed; fetch the current task and resubmit")]
    RoundClosed,
    /// The caller's streamed update exceeded `max_update_bytes` before
    /// it finished arriving, and the transport stopped reading rather
    /// than keep growing a buffer.
    ///
    /// Distinct from `Other` because it is the one submission failure a
    /// client can act on *without* an operator: the update really is too
    /// big for this deployment's configured bound, and either the client
    /// is sending the wrong model or the bound needs raising. It also
    /// maps to `resource_exhausted` rather than `internal`, which is
    /// what tells an honest client "back off / this won't succeed on
    /// retry" instead of "the server is broken".
    #[error(
        "client {client_id}'s update exceeded the {limit_bytes}-byte limit \
         (at least {received_bytes} bytes received before the stream was cut)"
    )]
    UpdateTooLarge {
        /// Whose stream was cut. Taken from the first chunk that carried
        /// a `client_id`, so it is what the caller claimed to be — the
        /// bound is enforced before any identity check, deliberately, or
        /// the check itself would be the thing being flooded.
        client_id: String,
        /// The configured `max_update_bytes` this stream exceeded.
        limit_bytes: u64,
        /// How much had accumulated when the limit tripped. A lower
        /// bound on what the client intended to send, not a total.
        received_bytes: u64,
    },
    #[error("dispatch failed: {0}")]
    /// Anything the variants above don't cover. Maps to
    /// `Status::internal`, so it should not carry a condition a client
    /// could act on — those deserve their own variant.
    Other(String),
}

impl From<DispatchError> for Status {
    fn from(err: DispatchError) -> Self {
        match err {
            DispatchError::UnknownClient(id) => Status::not_found(format!("unknown client {id}")),
            DispatchError::NotAllowed(id) => {
                Status::permission_denied(format!("client {id} is not on the node allow-list"))
            }
            // `unauthenticated`, not `permission_denied`: gRPC draws
            // exactly the distinction above — 16 means the credential
            // was missing or bad, 7 means the credential was fine and
            // the caller still isn't authorized.
            DispatchError::Unauthenticated(msg) => Status::unauthenticated(msg),
            DispatchError::RoundClosed => Status::failed_precondition(
                "this round already closed; fetch the current task and resubmit",
            ),
            // 8, not 3 (`invalid_argument`): the request was well-formed,
            // the server simply refused to hold that much of it.
            err @ DispatchError::UpdateTooLarge { .. } => {
                Status::resource_exhausted(err.to_string())
            }
            DispatchError::Other(msg) => Status::internal(msg),
        }
    }
}

/// What `FlTransportService` calls into to actually handle each RPC.
///
/// `#[async_trait]`: trait methods can't natively return
/// `impl Future` and still be object-safe (`Arc<dyn RoundDispatcher>`,
/// which `FlTransportService` holds) — this macro rewrites each `async
/// fn` into one returning a boxed, pinned future, which *is*
/// object-safe. `tonic`'s own generated `FlTransport` server trait uses
/// the same macro for the same reason.
#[async_trait::async_trait]
pub trait RoundDispatcher: Send + Sync + 'static {
    /// Answers pull mode's request for the current round's task.
    async fn fetch_task(&self, client_id: &str) -> Result<TaskResponse, DispatchError>;
    /// Hands back a stream of pushed tasks. Returns as soon as the
    /// subscription exists, not when the first task arrives.
    async fn subscribe_tasks(&self, client_id: &str) -> Result<TaskStream, DispatchError>;
    /// Accepts one client's complete update. The transport gathers the
    /// whole stream before calling this.
    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError>;
    /// `peer_cert_fingerprint` is `Some` only when the connection used
    /// mTLS and the server verified a client cert (see
    /// `peer_cert_fingerprint` in this crate) — `None` is the normal case
    /// for a `SharedToken`-based deployment or a non-TLS hop (e.g. the
    /// local loopback connection between `conflux-node` and its
    /// `ClientApp`, which never uses TLS), not an error.
    async fn register(
        &self,
        client_id: &str,
        auth_token: &str,
        peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError>;
    /// Records a liveness ping.
    async fn heartbeat(&self, client_id: &str) -> Result<HeartbeatResponse, DispatchError>;
}
