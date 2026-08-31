//! The admin token, enforced against a real router (Phase S3).
//!
//! `admin_auth.rs`'s own unit tests cover the startup decision and the
//! token comparison. These drive actual HTTP requests through the real
//! `Router`, because the thing most likely to go wrong is not the policy
//! but the wiring: a middleware attached below `with_state`, or applied
//! to a nested router, silently protects nothing while every unit test
//! still passes.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use conflux_config::{Mode, Overrides, Topology};
use conflux_server::{AdminToken, AppState};
use tower::ServiceExt;

const TOKEN: &str = "test-admin-token";

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

async fn request(
    token: Option<AdminToken>,
    method: &str,
    path: &str,
    auth_header: Option<&str>,
) -> StatusCode {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(value) = auth_header {
        builder = builder.header("authorization", value);
    }
    // Every mutating route here takes a JSON body; sending one
    // unconditionally keeps this helper uniform, and an unauthorized
    // request is rejected before the body is ever looked at.
    let req = builder
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"client_id":"x","identity":{"SharedToken":"y"}}"#,
        ))
        .unwrap();

    conflux_server::router(state(), token)
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

/// Every route except `/health`, with the method each accepts.
const PROTECTED: &[(&str, &str)] = &[
    ("GET", "/round/status"),
    ("POST", "/clients/register"),
    ("GET", "/admin/allowlist"),
    ("POST", "/admin/allowlist"),
    ("DELETE", "/admin/allowlist/some-client"),
];

#[tokio::test]
async fn without_a_token_every_route_is_rejected() {
    let token = Some(AdminToken::new(TOKEN));
    for (method, path) in PROTECTED {
        let status = request(token.clone(), method, path, None).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} was reachable with no Authorization header"
        );
    }
}

#[tokio::test]
async fn a_wrong_token_is_rejected_the_same_way_a_missing_one_is() {
    // Same status for both: an error distinguishing "absent" from
    // "wrong" tells an attacker which half of the guess was right.
    let token = Some(AdminToken::new(TOKEN));
    for (method, path) in PROTECTED {
        let status = request(token.clone(), method, path, Some("Bearer wrong-token")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
}

#[tokio::test]
async fn the_correct_token_is_accepted() {
    let token = Some(AdminToken::new(TOKEN));
    let header = format!("Bearer {TOKEN}");
    for (method, path) in PROTECTED {
        let status = request(token.clone(), method, path, Some(&header)).await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} rejected a valid token"
        );
    }
}

#[tokio::test]
async fn health_is_reachable_without_a_token() {
    // Liveness probes come from load balancers that cannot hold a
    // secret, and the endpoint reveals only that a process is running.
    let status = request(Some(AdminToken::new(TOKEN)), "GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn the_wrong_auth_scheme_is_rejected() {
    let token = Some(AdminToken::new(TOKEN));
    for header in [TOKEN, &format!("Basic {TOKEN}"), &format!("bearer {TOKEN}")] {
        let status = request(token.clone(), "GET", "/admin/allowlist", Some(header)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "accepted a non-Bearer Authorization header: {header:?}"
        );
    }
}

#[tokio::test]
async fn with_no_token_configured_every_route_stays_open() {
    // The pre-existing behavior, preserved for loopback development.
    // Only reachable in production because `validate_admin_binding`
    // refuses to start an exposed listener without a token.
    for (method, path) in PROTECTED {
        let status = request(None, method, path, None).await;
        assert_ne!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} demanded a token when none was configured"
        );
    }
}

#[tokio::test]
async fn the_allowlist_cannot_be_written_without_the_token() {
    // The specific attack this whole change exists to close: adding
    // yourself to the allow-list over HTTP, then presenting that
    // now-legitimate identity to the authenticated gRPC surface.
    let token = Some(AdminToken::new(TOKEN));
    let status = request(token, "POST", "/admin/allowlist", Some("Bearer guess")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
