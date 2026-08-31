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
//!
//! `CONFLUX_CONNECTION_MODE` (`push`/`pull`) picks which upstream
//! transport to open. It defaults to `pull`, which is *not* the same as
//! defaulting to any one topology's posture: three of the four topologies
//! resolve to pull, and a node started without being told anything about
//! its deployment should take the conservative option (ask when ready)
//! rather than hold a connection open on the assumption it's a trusted
//! silo. A `cross_silo` deployment sets this to `push` explicitly, the
//! same way it already has to be told the server address.

use std::net::SocketAddr;
use std::sync::Arc;

use conflux_net::{FlTransportService, PullTransport, PushTransport};
use conflux_node::{
    ClientAppKind, ConnectionMode, NodeBridge, RuntimeMode, validate_client_app_startup,
};
use conflux_privacy::GaussianClippingPrivacy;
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

    let connection_mode = match std::env::var("CONFLUX_CONNECTION_MODE").as_deref() {
        Ok("push") => ConnectionMode::Push,
        _ => ConnectionMode::Pull,
    };

    // Registration is identical in both modes — only the transport that
    // carries it differs — but it has to happen on the same transport the
    // node will go on using, not a throwaway one, so it lives inside each
    // branch rather than before them.
    // Phase 17: optional local DP, read from env vars for the same
    // reason `startup_guard.rs` reads its own — `conflux-node` calls no
    // `conflux-config` API directly (Phase 6's scope decision), so the
    // few values it needs are read here and their builtin fallbacks
    // mirrored inline. These names and defaults match
    // `conflux-config`'s `client_side_privacy_transform` (false),
    // `clip_norm` (1.0), and `noise_multiplier` (1.0) exactly.
    let client_side_privacy = matches!(
        std::env::var("CONFLUX_CLIENT_SIDE_PRIVACY_TRANSFORM").as_deref(),
        Ok("true")
    );
    let local_privacy = client_side_privacy.then(|| {
        fn env_f32(name: &str, default: f32) -> f32 {
            std::env::var(name)
                .ok()
                .map(|v| {
                    v.parse()
                        .unwrap_or_else(|_| panic!("{name}={v:?} is not a valid number"))
                })
                .unwrap_or(default)
        }
        GaussianClippingPrivacy {
            clip_norm: env_f32("CONFLUX_CLIP_NORM", 1.0),
            noise_multiplier: env_f32("CONFLUX_NOISE_MULTIPLIER", 1.0),
        }
    });
    let privacy_seed: Option<u64> = std::env::var("CONFLUX_SEED_VALUE")
        .ok()
        .map(|v| v.parse().expect("CONFLUX_SEED_VALUE must be an integer"));

    let bridge = match connection_mode {
        ConnectionMode::Pull => {
            let mut upstream = PullTransport::connect(server_addr)
                .await
                .expect("failed to connect to conflux-server");
            upstream
                .register(&client_id, "node-auth-token")
                .await
                .expect("failed to register with conflux-server");
            let bridge = NodeBridge::new(upstream, client_id.clone());
            Arc::new(apply_local_privacy(bridge, local_privacy, privacy_seed))
        }
        ConnectionMode::Push => {
            let mut upstream = PushTransport::connect(server_addr)
                .await
                .expect("failed to connect to conflux-server");
            upstream
                .register(&client_id, "node-auth-token")
                .await
                .expect("failed to register with conflux-server");
            let bridge = NodeBridge::new_push(upstream, client_id.clone());
            Arc::new(apply_local_privacy(bridge, local_privacy, privacy_seed))
        }
    };
    tracing::info!(
        %client_id,
        connection_mode = connection_mode.as_str(),
        client_side_privacy_transform = client_side_privacy,
        "registered with conflux-server"
    );

    let listener = tokio::net::TcpListener::bind(local_addr)
        .await
        .expect("failed to bind local listener");
    tracing::info!(
        local_addr = %listener.local_addr().unwrap(),
        "local gRPC server listening for the Python ClientApp"
    );

    tonic::transport::Server::builder()
        .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown_signal())
        .await
        .expect("local grpc server failed");
    tracing::info!("local grpc server stopped; node exiting");
}

/// Resolves when the process is asked to stop: Ctrl-C on any platform, or
/// `SIGTERM` on Unix (Tier 5, H3).
///
/// The node's shutdown is simpler than the server's — it holds no round
/// state and writes no checkpoints, so there is nothing to drain. What it
/// gains is an *orderly* stop: the local listener closes rather than the
/// process vanishing, so a Python `ClientApp` mid-call sees a closed
/// connection instead of a reset, and `docker stop` produces exit code 0
/// rather than a signal death.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not install SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => tracing::info!("received Ctrl-C; shutting down"),
        _ = terminate => tracing::info!("received SIGTERM; shutting down"),
    }
}

/// Applies the client-side privacy transform to `bridge` when one was
/// configured. A free function rather than inline in both match arms —
/// the two arms build different upstreams but need identical treatment
/// from here on, and duplicating the `if let` would be the kind of
/// near-copy where one branch quietly stops matching the other.
fn apply_local_privacy(
    bridge: NodeBridge,
    mechanism: Option<GaussianClippingPrivacy>,
    seed: Option<u64>,
) -> NodeBridge {
    match mechanism {
        Some(mechanism) => {
            tracing::info!(
                clip_norm = mechanism.clip_norm,
                noise_multiplier = mechanism.noise_multiplier,
                seeded = seed.is_some(),
                "client-side privacy transform enabled; updates are clipped and noised \
                 before leaving this node"
            );
            bridge.with_local_privacy(mechanism, seed)
        }
        None => bridge,
    }
}
