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

#![warn(missing_docs)]

mod client;
mod dispatcher;
pub mod jwt;
mod peer_identity;
mod service;
pub mod tls;

pub use client::{PullTransport, PushTransport};
pub use dispatcher::{DispatchError, RoundDispatcher, TaskStream};
pub use peer_identity::peer_cert_fingerprint;
pub use service::{DEFAULT_MAX_UPDATE_BYTES, FlTransportService};

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
}
