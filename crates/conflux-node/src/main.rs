//! Client binary — Rust-side networking/orchestration, hands training off
//! to the Python `ClientApp` over local loopback gRPC.
//!
//! See `docs/spec/conflux-spec-v1.md` §7, §10 (Phase 6). No CLI/config
//! resolution yet — address/id come from env vars, matching
//! `conflux-server`'s Phase 5 `main.rs`.
//!
//! Phase 9b: `CONFLUX_MODE`/`CONFLUX_ALLOW_STUB_CLIENT`/
//! `CONFLUX_CLIENT_APP_KIND` gate startup via `startup_guard` — see that
//! module and `docs/phases/phase-9b-stub-client-guard.md`.

use std::net::SocketAddr;
use std::sync::Arc;

use conflux_net::{FlTransportService, PullTransport};
use conflux_node::{ClientAppKind, NodeBridge, RuntimeMode, validate_client_app_startup};
use conflux_proto::fl_transport_server::FlTransportServer;
use tokio_stream::wrappers::TcpListenerStream;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mode = match std::env::var("CONFLUX_MODE").as_deref() {
        Ok("production") => RuntimeMode::Production,
        _ => RuntimeMode::Research,
    };
    // Mirrors `conflux-config::Mode::defaults().allow_stub_client`'s own
    // per-mode default (research=true, production=false) — kept inline
    // here rather than a `conflux-config` dependency, see
    // `startup_guard.rs`'s module doc comment.
    let allow_stub_client = match std::env::var("CONFLUX_ALLOW_STUB_CLIENT").as_deref() {
        Ok("true") => true,
        Ok("false") => false,
        _ => mode == RuntimeMode::Research,
    };
    let client_app_kind = match std::env::var("CONFLUX_CLIENT_APP_KIND").as_deref() {
        Ok("real") => ClientAppKind::Real,
        // Default "stub" matches what's actually shipped today
        // (`python/conflux_client/stub_client.py`) — see
        // docs/adr/0005-python-sdk-deferred.md.
        _ => ClientAppKind::Stub,
    };
    validate_client_app_startup(mode, allow_stub_client, client_app_kind)
        .expect("client app startup guard failed");

    let server_addr = std::env::var("CONFLUX_SERVER_ADDR")
        .unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());
    let client_id = std::env::var("CONFLUX_CLIENT_ID").unwrap_or_else(|_| "node-1".to_string());
    let local_addr: SocketAddr = std::env::var("CONFLUX_LOCAL_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:47100".to_string())
        .parse()
        .expect("invalid CONFLUX_LOCAL_ADDR");

    let mut upstream = PullTransport::connect(server_addr)
        .await
        .expect("failed to connect to conflux-server");
    upstream
        .register(&client_id, "node-auth-token")
        .await
        .expect("failed to register with conflux-server");
    tracing::info!(%client_id, "registered with conflux-server");

    let bridge = Arc::new(NodeBridge::new(upstream, client_id));

    let listener = tokio::net::TcpListener::bind(local_addr)
        .await
        .expect("failed to bind local listener");
    tracing::info!(
        local_addr = %listener.local_addr().unwrap(),
        "local gRPC server listening for the Python ClientApp"
    );

    tonic::transport::Server::builder()
        .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
        .serve_with_incoming(TcpListenerStream::new(listener))
        .await
        .expect("local grpc server failed");
}
