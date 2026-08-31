//! Tier 5 (H2): `/health` must report the round loop, not a constant.
//!
//! The defect these tests exist to prevent: the round loop ran in its own
//! task and `break`d on any error but `EmptyBatch`, so a single transient
//! backend failure ended the experiment permanently — while the gRPC and
//! HTTP servers kept serving and `/health` kept returning a hardcoded
//! `"ok"`. An orchestrator saw a healthy pod doing no work, indefinitely.
//!
//! Two rules encoded here:
//!
//! 1. **A retryable failure must not stop the experiment.** The line
//!    between retryable and fatal is `ServerError::is_transient`.
//! 2. **`/health` must be able to say the loop is dead.** A health endpoint
//!    that cannot report the failure of the thing the process exists to do
//!    is not a health endpoint.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use conflux_config::{Mode, Overrides, Topology};
use conflux_server::{AppState, RoundLoopState, ServerError};
use tower::ServiceExt;

fn state() -> Arc<AppState> {
    let config = conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        None,
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap();
    Arc::new(AppState::new(config, vec![0.0]))
}

/// Drives a real request through the real router and returns status + body.
async fn get_health(state: Arc<AppState>) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let response = conflux_server::router(state, None)
        .oneshot(req)
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

// ---------------------------------------------------------------------
// Rule 1 — the transient/fatal split
// ---------------------------------------------------------------------

/// Backend I/O is retryable. This is the case the old behavior got most
/// wrong: a Redis reconnect ended the experiment.
#[test]
fn backend_failures_are_transient() {
    let registry = ServerError::Registry(conflux_registry::RegistryError::Backend(
        "connection refused".to_string(),
    ));
    assert!(
        registry.is_transient(),
        "a registry outage must not end the experiment — it may be back next round"
    );

    let store = ServerError::Store(conflux_store::StoreError::Backend(
        "connection reset by peer".to_string(),
    ));
    assert!(store.is_transient());
}

/// An aggregation rejection describes one batch, not the experiment. The
/// client that sent a `NaN` may not be selected next round; if it is, the
/// rejection is doing its job every time.
#[test]
fn aggregation_rejections_are_transient() {
    assert!(
        ServerError::Aggregator(conflux_core::AggregatorError::EmptyBatch).is_transient(),
        "the ordinary 'nobody has registered yet' case must be retryable"
    );
    assert!(
        ServerError::Aggregator(conflux_core::AggregatorError::NonFiniteWeights {
            client_id: "attacker".to_string(),
            index: 0,
        })
        .is_transient(),
        "one client's bad batch must not end the experiment for everyone else"
    );
}

/// The one case where stopping is the *specified* behavior rather than a
/// failure to handle something. `budget_exhausted_action = halt` means
/// halt, and no amount of waiting produces more epsilon (ADR 0006).
#[test]
fn an_exhausted_privacy_budget_is_fatal_in_both_scopes() {
    assert!(
        !ServerError::BudgetExhausted.is_transient(),
        "retrying an exhausted budget would silently violate the privacy guarantee"
    );
    assert!(
        !ServerError::BudgetExhaustedForClient {
            client_id: "c1".to_string(),
        }
        .is_transient()
    );
}

// ---------------------------------------------------------------------
// Rule 2 — /health reports the loop
// ---------------------------------------------------------------------

/// A server whose loop has not completed a round yet is healthy and says
/// `starting` — not `running`, which would be a small lie, and not
/// unhealthy, which would fail a readiness probe on every cold start.
#[tokio::test]
async fn a_fresh_server_reports_starting_and_returns_200() {
    let (status, body) = get_health(state()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert_eq!(body["round_loop"], "starting");
    assert_eq!(body["last_completed_round"], 0);
    assert_eq!(body["consecutive_failures"], 0);
    assert!(
        body.get("last_error").is_none(),
        "no error should be reported when there hasn't been one"
    );
}

/// The success path: a completed round is visible, by number.
#[tokio::test]
async fn a_completed_round_is_reported() {
    let state = state();
    state.round_loop_health.record_success(12);

    let (status, body) = get_health(state).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["round_loop"], "running");
    assert_eq!(body["last_completed_round"], 12);
}

/// The distinction the endpoint exists to draw. A loop that is retrying is
/// still **200**: restarting the process would not fix an unreachable
/// Redis, it would only add a cold start to the outage. But the failure
/// count and the error are reported, so an operator can see it.
#[tokio::test]
async fn a_retrying_loop_is_degraded_but_still_healthy() {
    let state = state();
    state
        .round_loop_health
        .record_transient_failure("redis: connection refused");
    state
        .round_loop_health
        .record_transient_failure("redis: connection refused");

    let (status, body) = get_health(state).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a retrying loop must not fail a health check — that turns a backend \
         outage into a crash loop on top of a backend outage"
    );
    assert_eq!(body["status"], "ok");
    assert_eq!(body["round_loop"], "degraded");
    assert_eq!(body["consecutive_failures"], 2);
    assert_eq!(body["last_error"], "redis: connection refused");
}

/// The case that was previously invisible, and the whole reason for H2. A
/// stopped loop must return 503 so an orchestrator acts on it.
#[tokio::test]
async fn a_stopped_loop_returns_503_and_says_why() {
    let state = state();
    state
        .round_loop_health
        .record_stopped(Some("privacy budget exhausted for this experiment"));

    let (status, body) = get_health(state).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a server whose round loop has stopped must not report itself healthy"
    );
    assert_eq!(body["status"], "unhealthy");
    assert_eq!(body["round_loop"], "stopped");
    assert_eq!(
        body["last_error"],
        "privacy budget exhausted for this experiment"
    );
}

/// Recovery has to be reported too — a loop that comes back must stop
/// showing the error it recovered from, or the endpoint becomes noise an
/// operator learns to ignore.
#[tokio::test]
async fn recovery_clears_the_degraded_state() {
    let state = state();
    state
        .round_loop_health
        .record_transient_failure("postgres down");
    state.round_loop_health.record_success(3);

    let (status, body) = get_health(state.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["round_loop"], "running");
    assert_eq!(body["consecutive_failures"], 0);
    assert!(body.get("last_error").is_none());
    assert_eq!(state.round_loop_health.state(), RoundLoopState::Running);
}

/// `/health` stays exempt from the admin token (Phase S3's policy) — a
/// health check that needs a credential is not one an orchestrator can
/// use. Guarded here because H2 changed the handler's signature, which is
/// exactly the kind of edit that can silently move a route under a layer.
#[tokio::test]
async fn health_is_still_reachable_without_the_admin_token() {
    let req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    let response = conflux_server::router(
        state(),
        Some(conflux_server::AdminToken::new("a-real-token".to_string())),
    )
    .oneshot(req)
    .await
    .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "/health must not require the admin token"
    );
}
