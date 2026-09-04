//! Minimal HTTP admin surface, served on a separate port from the gRPC
//! `FlTransport` service.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use conflux_registry::{ClientId, NodeAllowlist, NodeIdentity, Registry, RegistryError};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::admin_auth::{AdminToken, require_admin_token};

/// Builds the admin router.
///
/// `admin_token` gates every route except `/health` — see
/// `admin_auth` (private module) for the policy and why `/health` is exempt.
/// `None` leaves the surface open, which is only reachable for a
/// loopback-bound listener because `validate_admin_binding` refuses to
/// start otherwise.
pub fn router(state: Arc<AppState>, admin_token: Option<AdminToken>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/round/status", get(round_status))
        .route("/clients/register", post(register))
        .route("/admin/allowlist", get(list_allowlist).post(allow_node))
        .route("/admin/allowlist/{client_id}", delete(revoke_node))
        .layer(axum::middleware::from_fn(move |req, next| {
            require_admin_token(admin_token.clone(), req, next)
        }))
        .with_state(state)
}

/// What `/health` reports.
///
/// Not a constant: the round loop runs in its own task, so an endpoint
/// that only proved the process was accepting connections would carry on
/// saying it was fine after the loop stopped — true in the narrow sense,
/// and not at all in the sense anyone polling `/health` means.
#[derive(Serialize)]
struct HealthResponse {
    /// `"ok"` or `"unhealthy"`. Kept as the first field so a probe that
    /// matches on the bare string still sees it.
    status: &'static str,
    /// `starting`, `running`, `degraded`, or `stopped`.
    round_loop: &'static str,
    /// The last round that completed, or `0` if none has yet.
    last_completed_round: u64,
    /// Failed rounds since the last success. Non-zero means the loop is
    /// retrying, not that it has given up.
    consecutive_failures: u32,
    /// Why the loop is degraded or stopped. `None` when it is fine.
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

/// Returns `200` while the round loop is alive — **including while it is
/// degraded and retrying** — and `503` once it has stopped.
///
/// The distinction is deliberate and is the whole point of the endpoint: a
/// restart does not fix an unreachable Redis, so reporting a retrying loop
/// as unhealthy would turn a backend outage into a crash loop on top of a
/// backend outage. A *stopped* loop is the state a restart or a config
/// change is the only remedy for, so that is the one this reports.
async fn health(State(state): State<Arc<AppState>>) -> (StatusCode, Json<HealthResponse>) {
    let health = &state.round_loop_health;
    let loop_state = health.state();
    let code = if loop_state.is_healthy() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(HealthResponse {
            status: if loop_state.is_healthy() {
                "ok"
            } else {
                "unhealthy"
            },
            round_loop: loop_state.as_str(),
            last_completed_round: health.last_completed_round(),
            consecutive_failures: health.consecutive_failures(),
            last_error: health.last_error(),
        }),
    )
}

#[derive(Serialize)]
struct RoundStatusResponse {
    round: u64,
}

async fn round_status(State(state): State<Arc<AppState>>) -> Json<RoundStatusResponse> {
    Json(RoundStatusResponse {
        round: state.round.load(Ordering::SeqCst),
    })
}

#[derive(Deserialize)]
struct RegisterHttpRequest {
    client_id: String,
}

#[derive(Serialize)]
struct RegisterHttpResponse {
    accepted: bool,
}

/// Delegates to the same registry the gRPC `Register` RPC uses — this is
/// an admin/observability entry point, not a second source of truth.
///
/// **It is not a second *authentication* path either, and that is worth
/// being explicit about.** The gRPC `Register` RPC runs JWT verification
/// and the node allow-list check; this handler runs neither. Without the
/// admin token it would be a way to register a client while bypassing
/// both — the HTTP port undoing the gRPC port's authentication. It sits
/// behind the admin token, so reaching it already requires an operator
/// credential, which is the only footing on which "register this client,
/// no questions asked" is a reasonable thing to offer.
async fn register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterHttpRequest>,
) -> Json<RegisterHttpResponse> {
    let result = state.registry.register(ClientId(req.client_id)).await;
    let accepted = matches!(result, Ok(()) | Err(RegistryError::AlreadyRegistered(_)));
    Json(RegisterHttpResponse { accepted })
}

/// The allow-list admin surface: the operator's way to populate the
/// allow-list `dispatcher.rs`'s `register()` enforces (when
/// `config.require_node_auth` is on).
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum NodeIdentityHttp {
    CertFingerprint { fingerprint: String },
    SharedToken { token: String },
}

impl From<NodeIdentityHttp> for NodeIdentity {
    fn from(value: NodeIdentityHttp) -> Self {
        match value {
            NodeIdentityHttp::CertFingerprint { fingerprint } => {
                NodeIdentity::CertFingerprint(fingerprint)
            }
            NodeIdentityHttp::SharedToken { token } => NodeIdentity::SharedToken(token),
        }
    }
}

#[derive(Deserialize)]
struct AllowNodeRequest {
    client_id: String,
    identity: NodeIdentityHttp,
}

#[derive(Serialize)]
struct AllowlistOpResponse {
    ok: bool,
}

async fn allow_node(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AllowNodeRequest>,
) -> (StatusCode, Json<AllowlistOpResponse>) {
    match state
        .node_allowlist
        .allow(ClientId(req.client_id), req.identity.into())
        .await
    {
        Ok(()) => (StatusCode::OK, Json(AllowlistOpResponse { ok: true })),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(AllowlistOpResponse { ok: false }),
        ),
    }
}

async fn revoke_node(
    State(state): State<Arc<AppState>>,
    Path(client_id): Path<String>,
) -> (StatusCode, Json<AllowlistOpResponse>) {
    match state.node_allowlist.revoke(&ClientId(client_id)).await {
        Ok(()) => (StatusCode::OK, Json(AllowlistOpResponse { ok: true })),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(AllowlistOpResponse { ok: false }),
        ),
    }
}

#[derive(Serialize)]
struct AllowlistListResponse {
    client_ids: Vec<String>,
}

async fn list_allowlist(
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<AllowlistListResponse>) {
    match state.node_allowlist.list().await {
        Ok(ids) => (
            StatusCode::OK,
            Json(AllowlistListResponse {
                client_ids: ids.into_iter().map(|id| id.0).collect(),
            }),
        ),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(AllowlistListResponse {
                client_ids: Vec::new(),
            }),
        ),
    }
}
