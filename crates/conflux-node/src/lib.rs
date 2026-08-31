//! Client binary's library half — `NodeBridge`, the local hop's
//! `RoundDispatcher` implementation. `main.rs` is a thin wrapper around
//! this.
//!
//! See `docs/spec/conflux-spec-v1.md` §7, §10 (Phase 6).

#![warn(missing_docs)]

mod bridge;
mod startup_guard;

pub use bridge::{ConnectionMode, NodeBridge};
pub use startup_guard::{
    ClientAppKind, RuntimeMode, StartupGuardError, validate_client_app_startup,
};
