//! Client binary's library half — `NodeBridge`, the local hop's
//! `RoundDispatcher` implementation. `main.rs` is a thin wrapper around
//! this.
//!
//! # Example
//!
//! The startup guard is the one piece worth seeing on its own: a
//! production node must not silently train with the stub `ClientApp`,
//! which returns fixed dummy weights and never imports PyTorch. It lives
//! here rather than in `conflux-server` because only `conflux-node` has
//! the local loopback listener a `ClientApp` connects to.
//!
//! ```
//! use conflux_node::{ClientAppKind, RuntimeMode, validate_client_app_startup};
//!
//! // Research with the stub: the normal development path.
//! assert!(validate_client_app_startup(
//!     RuntimeMode::Research,
//!     false,
//!     ClientAppKind::Stub,
//! )
//! .is_ok());
//!
//! // Production with the stub: refused. Failing to start is the correct
//! // response to a deployment that would train on dummy weights.
//! assert!(validate_client_app_startup(
//!     RuntimeMode::Production,
//!     false,
//!     ClientAppKind::Stub,
//! )
//! .is_err());
//!
//! // ...unless it is overridden explicitly, which is what makes it a
//! // deliberate choice rather than an accident.
//! assert!(validate_client_app_startup(
//!     RuntimeMode::Production,
//!     true,
//!     ClientAppKind::Stub,
//! )
//! .is_ok());
//!
//! // A real ClientApp in production needs no override at all.
//! assert!(validate_client_app_startup(
//!     RuntimeMode::Production,
//!     false,
//!     ClientAppKind::Real,
//! )
//! .is_ok());
//! ```

#![warn(missing_docs)]

mod bridge;
mod startup_guard;

pub use bridge::{ConnectionMode, NodeBridge};
pub use startup_guard::{
    ClientAppKind, RuntimeMode, StartupGuardError, validate_client_app_startup,
};
