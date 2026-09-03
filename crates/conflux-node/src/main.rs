//! Client binary — Rust-side networking/orchestration, hands training off
//! to a local `ClientApp` (Python or Rust) over loopback gRPC.
//!
//! No CLI/config resolution — address/id come from env vars, matching
//! `conflux-server`'s `main.rs`.
//!
//! `CONFLUX_MODE`/`CONFLUX_ALLOW_STUB_CLIENT`/
//! `CONFLUX_CLIENT_APP_KIND` gate startup via `startup_guard` — see that
//! module.
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
// `ClientTlsConfig` is available here without `conflux-node` enabling a
// tonic TLS feature of its own: `conflux-net` enables `tls-aws-lc`, and
// Cargo unifies features across the workspace, so the one compiled `tonic`
// this binary links already has it.
use tonic::transport::ClientTlsConfig;

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
        // Default "stub" matches the shipped placeholder
        // (`python/conflux_client/stub_client.py`).
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
    // Optional local DP, read from env vars for the same reason
    // `startup_guard.rs` reads its own — `conflux-node` calls no
    // `conflux-config` API directly, so the few values it needs are
    // read here and their builtin fallbacks mirrored inline. These
    // names and defaults match
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

    // The credential this node presents at registration. Defaults to the
    // legacy placeholder so an allow-list-by-id deployment on a trusted
    // network keeps working unchanged; set it to a real per-client token or
    // JWT for a server enforcing `require_node_auth` / `auth = "jwt"`.
    let auth_token =
        std::env::var("CONFLUX_NODE_AUTH_TOKEN").unwrap_or_else(|_| "node-auth-token".to_string());

    // Optional client-side TLS, resolved from `CONFLUX_TLS_*` env into one
    // of three postures — plaintext, server-authenticated (CA + domain), or
    // mutual (all four vars) — see `resolve_client_tls`.
    let client_tls = resolve_client_tls();
    let tls_mode = client_tls.label();
    let custom_auth_token = auth_token != "node-auth-token";

    let bridge = match connection_mode {
        ConnectionMode::Pull => {
            let mut upstream = match client_tls.config() {
                Some(tls) => PullTransport::connect_with_tls(server_addr, tls).await,
                None => PullTransport::connect(server_addr).await,
            }
            .expect("failed to connect to conflux-server");
            upstream
                .register(&client_id, &auth_token)
                .await
                .expect("failed to register with conflux-server");
            let bridge = NodeBridge::new(upstream, client_id.clone());
            Arc::new(apply_local_privacy(bridge, local_privacy, privacy_seed))
        }
        ConnectionMode::Push => {
            let mut upstream = match client_tls.config() {
                Some(tls) => PushTransport::connect_with_tls(server_addr, tls).await,
                None => PushTransport::connect(server_addr).await,
            }
            .expect("failed to connect to conflux-server");
            upstream
                .register(&client_id, &auth_token)
                .await
                .expect("failed to register with conflux-server");
            let bridge = NodeBridge::new_push(upstream, client_id.clone());
            Arc::new(apply_local_privacy(bridge, local_privacy, privacy_seed))
        }
    };
    tracing::info!(
        %client_id,
        connection_mode = connection_mode.as_str(),
        tls = tls_mode,
        custom_auth_token,
        client_side_privacy_transform = client_side_privacy,
        "registered with conflux-server"
    );

    let listener = tokio::net::TcpListener::bind(local_addr)
        .await
        .expect("failed to bind local listener");
    tracing::info!(
        local_addr = %listener.local_addr().unwrap(),
        "local gRPC server listening for the ClientApp"
    );

    tonic::transport::Server::builder()
        .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown_signal())
        .await
        .expect("local grpc server failed");
    tracing::info!("local grpc server stopped; node exiting");
}

/// Resolves when the process is asked to stop: Ctrl-C on any platform, or
/// `SIGTERM` on Unix.
///
/// The node's shutdown is simpler than the server's — it holds no round
/// state and writes no checkpoints, so there is nothing to drain. What it
/// gains is an *orderly* stop: the local listener closes rather than the
/// process vanishing, so a `ClientApp` mid-call sees a closed
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

/// The node's client-side TLS posture, resolved from `CONFLUX_TLS_*` env.
enum ClientTls {
    /// No TLS — plaintext, for the local loopback or a trusted network.
    Plaintext,
    /// Server-authenticated TLS with no client certificate; the node's
    /// identity travels in its registration token/JWT instead.
    ServerAuth(ClientTlsConfig),
    /// Mutual TLS — the node presents its own certificate as identity.
    Mutual(ClientTlsConfig),
}

impl ClientTls {
    /// The tonic config to connect with, or `None` for a plaintext hop.
    fn config(&self) -> Option<ClientTlsConfig> {
        match self {
            ClientTls::Plaintext => None,
            ClientTls::ServerAuth(c) | ClientTls::Mutual(c) => Some(c.clone()),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            ClientTls::Plaintext => "off",
            ClientTls::ServerAuth(_) => "server-auth",
            ClientTls::Mutual(_) => "mutual",
        }
    }
}

/// Resolves the TLS posture from `CONFLUX_TLS_*` env:
///
/// - nothing set ⇒ [`ClientTls::Plaintext`];
/// - `SERVER_CA_PATH` + `DOMAIN` only ⇒ [`ClientTls::ServerAuth`] — encrypted
///   and the server verified, with the node's identity carried by its
///   token/JWT rather than a client certificate;
/// - all four (adding `CLIENT_CERT_PATH` + `CLIENT_KEY_PATH`) ⇒
///   [`ClientTls::Mutual`] — the node presents its own certificate.
///
/// Anything else is a startup panic: a half-set security control must fail
/// loudly, never silently downgrade to plaintext.
fn resolve_client_tls() -> ClientTls {
    let cert = std::env::var("CONFLUX_TLS_CLIENT_CERT_PATH").ok();
    let key = std::env::var("CONFLUX_TLS_CLIENT_KEY_PATH").ok();
    let ca = std::env::var("CONFLUX_TLS_SERVER_CA_PATH").ok();
    let domain = std::env::var("CONFLUX_TLS_DOMAIN").ok();

    let read = |path: &str, what: &str| {
        std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {what} at {path:?}: {e}"))
    };

    match (cert, key, ca, domain) {
        (None, None, None, None) => ClientTls::Plaintext,
        (None, None, Some(ca), Some(domain)) => {
            ClientTls::ServerAuth(conflux_net::tls::client_tls_config_server_auth(
                &read(&ca, "CONFLUX_TLS_SERVER_CA_PATH"),
                &domain,
            ))
        }
        (Some(cert), Some(key), Some(ca), Some(domain)) => {
            ClientTls::Mutual(conflux_net::tls::client_tls_config(
                &read(&cert, "CONFLUX_TLS_CLIENT_CERT_PATH"),
                &read(&key, "CONFLUX_TLS_CLIENT_KEY_PATH"),
                &read(&ca, "CONFLUX_TLS_SERVER_CA_PATH"),
                &domain,
            ))
        }
        _ => panic!(
            "invalid TLS config. Set either nothing (plaintext); \
             CONFLUX_TLS_SERVER_CA_PATH + CONFLUX_TLS_DOMAIN (server-authenticated TLS); \
             or all four, adding CONFLUX_TLS_CLIENT_CERT_PATH + CONFLUX_TLS_CLIENT_KEY_PATH (mTLS)"
        ),
    }
}
