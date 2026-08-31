//! `conflux-server`'s library half: `AppState`, the `RoundDispatcher`
//! impl, the round pipeline, and the HTTP admin surface. `main.rs` is a
//! thin wrapper around this.
//!
//! See `docs/spec/conflux-spec-v1.md` §8, §10 (Phase 5).

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
