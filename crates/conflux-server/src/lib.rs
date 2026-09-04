//! `conflux-server`'s library half: `AppState`, the `RoundDispatcher`
//! impl, the round pipeline, and the HTTP admin surface. `main.rs` is a
//! thin wrapper around this.
//!
//! # Example
//!
//! One round, driven directly. `run_round` is the whole pipeline —
//! select, dispatch, buffer, privacy, reputation, aggregate, checkpoint
//! — and returns a summary of what actually happened rather than just
//! succeeding silently.
//!
//! ```no_run
//! use conflux_config::{Mode, Overrides, Topology};
//! use conflux_server::{AppState, run_round};
//! use std::sync::Arc;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let config = conflux_config::resolve(
//!     Topology::CrossDevice,
//!     Mode::Research,
//!     None,
//!     &Overrides::default(),
//!     &Overrides { aggregator: Some("krum".into()), ..Default::default() },
//! )?;
//!
//! // One process, one experiment — there is no tenant dimension here.
//! // Running two experiments means two processes.
//! let state = Arc::new(AppState::new(config, vec![0.0; 3]));
//!
//! let summary = run_round(&state).await?;
//! println!(
//!     "round {} closed on {:?}: {} selected, {} submitted, {} aggregated",
//!     summary.round,
//!     summary.flush_reason,
//!     summary.num_selected,
//!     summary.num_submitted,
//!     summary.num_passed,
//! );
//! # Ok(())
//! # }
//! ```
//!
//! `conflux-server` never names an aggregator itself — it asks
//! `conflux-core` to build whatever `config.aggregator.value` says, which
//! is why adding a method never touches this crate.

#![warn(missing_docs)]

mod admin_auth;
mod app_state;
mod auth_enforcement;
mod backend_selection;
mod dispatcher;
mod error;
mod http;
mod round;
mod round_health;

pub use admin_auth::{AdminAuthError, AdminToken, validate_admin_binding};
pub use app_state::{AppState, AppStateError};
pub use auth_enforcement::{
    AuthEnforcementError, TlsMaterial, resolve_server_tls, validate_jwt_startup,
    verify_jwt_if_required,
};
pub use backend_selection::{
    AccountingBackend, BackendSelection, BackendSelectionError, RegistryBackend, StoreBackend,
    validate_production_backends,
};
pub use error::ServerError;
pub use http::router;
pub use round::{RoundSummary, run_round};
pub use round_health::{RoundLoopHealth, RoundLoopState, backoff_secs};
