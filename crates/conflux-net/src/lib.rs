//! Transport — dual-mode (push/pull).
//!
//! See `docs/spec/conflux-spec-v1.md` §3.

mod client;
mod dispatcher;
mod peer_identity;
mod service;
pub mod tls;

pub use client::{PullTransport, PushTransport};
pub use dispatcher::{DispatchError, RoundDispatcher, TaskStream};
pub use peer_identity::peer_cert_fingerprint;
pub use service::FlTransportService;

/// The client-side transport error type — a connection failure or an RPC
/// that came back with a `tonic::Status`.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("failed to connect: {0}")]
    Connect(#[from] tonic::transport::Error),
    #[error("RPC failed: {0}")]
    Rpc(#[from] tonic::Status),
}
