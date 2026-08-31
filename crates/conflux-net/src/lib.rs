//! Transport — dual-mode (push/pull).
//!
//! Every Conflux FL deployment picks one of two ways for a client to learn
//! about its next training task: pull mode, where the client asks
//! ([`PullTransport::fetch_task`]) whenever it's ready, or push mode, where
//! the server streams tasks to a subscribed client as they become available
//! ([`PushTransport::subscribe_tasks`]). Which mode a deployment uses is a
//! configuration choice (`connection_mode`), not a code fork — this crate
//! ships both client-side transports plus the server-side
//! [`FlTransportService`] that answers either one through a single
//! [`RoundDispatcher`] trait.

//! # Example
//!
//! The client side of a round, in pull mode. `no_run`: this compiles
//! against the real API but needs a server listening to execute.
//!
//! ```no_run
//! use conflux_net::PullTransport;
//! use conflux_proto::{ClientDelta, DeltaChunk, decode_weights, encode_weights};
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = PullTransport::connect("http://127.0.0.1:50051").await?;
//! client.register("node-1", "my-auth-token").await?;
//!
//! // Ask for this round's task. The weights are an opaque little-endian
//! // f32 buffer — the server has no idea what architecture they describe
//! // (ADR 0004), which is the whole reason Python stays client-side.
//! let task = client.fetch_task("node-1").await?;
//! let weights = decode_weights(&task.model_weights)?;
//!
//! // ...train locally, producing an update of the same length...
//! let trained: Vec<f32> = weights.iter().map(|w| w * 0.99).collect();
//!
//! // Submissions are streamed as chunks, so the server can start
//! // reassembling before the last one lands.
//! client
//!     .submit_delta(vec![DeltaChunk {
//!         client_id: "node-1".to_string(),
//!         round: task.round,
//!         chunk_index: 0,
//!         total_chunks: 1,
//!         data: encode_weights(&trained),
//!         num_samples: 128,
//!         ..Default::default()
//!     }])
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! Push mode is the same crate, the same server, one different call —
//! the server streams tasks instead of answering requests:
//!
//! ```no_run
//! use conflux_net::PushTransport;
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let mut client = PushTransport::connect("http://127.0.0.1:50051").await?;
//! client.register("node-1", "my-auth-token").await?;
//!
//! let mut tasks = client.subscribe_tasks("node-1").await?;
//! # let _ = &mut tasks;
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

mod client;
mod dispatcher;
pub mod jwt;
mod peer_identity;
mod service;
pub mod tls;
mod trusted_reference;

pub use client::{PullTransport, PushTransport};
pub use dispatcher::{DispatchError, RoundDispatcher, TaskStream};
pub use peer_identity::peer_cert_fingerprint;
pub use service::{DEFAULT_MAX_UPDATE_BYTES, FlTransportService};
pub use trusted_reference::{SidecarCapabilities, TrustedReferenceTransport};

/// The client-side transport error type — a connection failure or an RPC
/// that came back with a `tonic::Status`.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("failed to connect: {0}")]
    /// The connection itself could not be established.
    Connect(#[from] tonic::transport::Error),
    #[error("RPC failed: {0}")]
    /// The connection worked; the RPC came back with an error status.
    Rpc(#[from] tonic::Status),
    #[error(
        "trusted-reference sidecar answered for round {got}, but round {expected} was asked \
         — a reference from the wrong round is a well-formed vector of the right length, so \
         using it would weaken the defense silently rather than fail"
    )]
    /// The sidecar's response carried a different round than the request
    /// (ADR 0011). Its own category rather than a generic `Rpc` error
    /// because the RPC *succeeded* — this is a correctness failure in a
    /// well-formed answer, which is exactly the kind that otherwise goes
    /// unnoticed.
    StaleSidecarResponse {
        /// The round the server asked about.
        expected: u64,
        /// The round the sidecar answered for.
        got: u64,
    },
}
