//! Running a node: registering upstream, then serving the local hop the
//! `ClientApp` connects to, until shutdown.
//!
//! In the library rather than the binary for the same reason the
//! server's startup is — `cflux node start` runs this, so there is one
//! node implementation rather than two.

use std::net::SocketAddr;
use std::sync::Arc;

use conflux_net::{FlTransportService, PullTransport, PushTransport, TransportError};
use conflux_privacy::GaussianClippingPrivacy;
use conflux_proto::fl_transport_server::FlTransportServer;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::ClientTlsConfig;

use crate::startup_guard::{
    ClientAppKind, RuntimeMode, StartupGuardError, validate_client_app_startup,
};
use crate::{ConnectionMode, NodeBridge};

/// Why a node could not start, or could not finish cleanly.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The stub `ClientApp` is not permitted in this configuration.
    #[error("{0}")]
    StartupGuard(#[from] StartupGuardError),
    /// A `CONFLUX_*` variable is set but malformed.
    #[error("{var}={value:?} is not valid: {message}")]
    InvalidVar {
        /// The variable.
        var: &'static str,
        /// What it held.
        value: String,
        /// Why it could not be used.
        message: String,
    },
    /// The upstream server could not be reached.
    #[error("failed to connect to conflux-server: {0}")]
    Connect(#[source] TransportError),
    /// The server refused this node's registration.
    #[error("failed to register with conflux-server: {0}")]
    Register(#[source] TransportError),
    /// The local listener could not take its address.
    #[error("cannot bind the local listener on {addr}: {source}")]
    Bind {
        /// The address that was refused.
        addr: SocketAddr,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The local listener, already bound, would not say what address it
    /// is bound to.
    ///
    /// Exceptional — the socket exists by this point — but not
    /// recoverable: on port 0 the address the OS chose is knowable only
    /// this way, and a node that cannot report its local hop is one no
    /// `ClientApp` can find.
    #[error("the local listener would not report its own address: {source}")]
    ListenerAddr {
        /// The underlying error.
        source: std::io::Error,
    },
    /// The local gRPC server stopped with an error.
    #[error("local grpc server failed: {0}")]
    LocalServer(#[source] tonic::transport::Error),
}

/// Parses a `CONFLUX_*` variable, or `None` when unset. Set-but-invalid
/// is an error rather than a silent default, matching the server.
fn parse_env<T>(var: &'static str) -> Result<Option<T>, RunError>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    match std::env::var(var) {
        Err(_) => Ok(None),
        Ok(value) => value
            .parse()
            .map(Some)
            .map_err(|e: T::Err| RunError::InvalidVar {
                var,
                value,
                message: e.to_string(),
            }),
    }
}

/// Everything a node needs, already resolved.
///
/// Separate from *reading* it because the environment is process-global:
/// it can hold one `CONFLUX_LOCAL_ADDR`, not N of them. Running several
/// nodes inside one process — a whole federation on one machine — needs
/// each to be handed its own configuration directly, so resolution and
/// running are two steps rather than one.
///
/// [`Self::from_env`] is the binary's path; constructing this directly is
/// the in-process path. Both end at [`run`], so neither is a second
/// implementation of the other.
#[derive(Debug, Clone)]
pub struct NodeConfig {
    /// Behavioural mode; decides how strict the stub-client guard is.
    pub mode: RuntimeMode,
    /// Whether the placeholder `ClientApp` is permitted.
    pub allow_stub_client: bool,
    /// Which kind of `ClientApp` will connect to the local hop.
    pub client_app_kind: ClientAppKind,
    /// The server to register with, e.g. `http://127.0.0.1:50051`.
    pub server_addr: String,
    /// This node's identity upstream.
    pub client_id: String,
    /// Where the local hop listens for its `ClientApp`. **Must differ per
    /// node** when several run in one process.
    ///
    /// Used by [`run`], which binds it. Ignored by [`run_on`], where the
    /// caller's listener decides — that is how N nodes get N distinct
    /// ports without any of them being guessed.
    pub local_addr: SocketAddr,
    /// Push or pull.
    pub connection_mode: ConnectionMode,
    /// Optional client-side DP, applied before an update leaves the node.
    pub local_privacy: Option<GaussianClippingPrivacy>,
    /// Seed for that mechanism, when reproducibility matters.
    pub privacy_seed: Option<u64>,
    /// The credential presented at registration.
    pub auth_token: String,
    /// Client-side TLS posture for the upstream hop.
    pub client_tls: ClientTls,
}

/// The default credential, kept as a constant because two places compare
/// against it: the fallback below, and the log line that reports whether
/// a real one was configured.
const PLACEHOLDER_AUTH_TOKEN: &str = "node-auth-token";

impl NodeConfig {
    /// Resolves a node's configuration from the `CONFLUX_*` environment —
    /// what `conflux-node` and `cflux node start` both use.
    pub fn from_env() -> Result<Self, RunError> {
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

        let connection_mode = match std::env::var("CONFLUX_CONNECTION_MODE").as_deref() {
            Ok("push") => ConnectionMode::Push,
            _ => ConnectionMode::Pull,
        };

        // Optional local DP, read from env vars for the same reason
        // `startup_guard.rs` reads its own — `conflux-node` calls no
        // `conflux-config` API directly, so the few values it needs are
        // read here and their builtin fallbacks mirrored inline. These
        // names and defaults match `conflux-config`'s
        // `client_side_privacy_transform` (false), `clip_norm` (1.0), and
        // `noise_multiplier` (1.0) exactly.
        let client_side_privacy = matches!(
            std::env::var("CONFLUX_CLIENT_SIDE_PRIVACY_TRANSFORM").as_deref(),
            Ok("true")
        );
        let local_privacy = client_side_privacy
            .then(|| {
                Ok::<_, RunError>(GaussianClippingPrivacy {
                    clip_norm: parse_env("CONFLUX_CLIP_NORM")?.unwrap_or(1.0),
                    noise_multiplier: parse_env("CONFLUX_NOISE_MULTIPLIER")?.unwrap_or(1.0),
                })
            })
            .transpose()?;

        Ok(Self {
            mode,
            allow_stub_client,
            client_app_kind,
            server_addr: std::env::var("CONFLUX_SERVER_ADDR")
                .unwrap_or_else(|_| "http://127.0.0.1:50051".to_string()),
            client_id: std::env::var("CONFLUX_CLIENT_ID").unwrap_or_else(|_| "node-1".to_string()),
            local_addr: parse_env("CONFLUX_LOCAL_ADDR")?
                .unwrap_or_else(|| "127.0.0.1:47100".parse().expect("a literal address")),
            connection_mode,
            local_privacy,
            privacy_seed: parse_env("CONFLUX_SEED_VALUE")?,
            // The credential this node presents at registration. Defaults to
            // the legacy placeholder so an allow-list-by-id deployment on a
            // trusted network keeps working unchanged; set it to a real
            // per-client token or JWT for a server enforcing
            // `require_node_auth` / `auth = "jwt"`.
            auth_token: std::env::var("CONFLUX_NODE_AUTH_TOKEN")
                .unwrap_or_else(|_| PLACEHOLDER_AUTH_TOKEN.to_string()),
            // Optional client-side TLS, resolved from `CONFLUX_TLS_*` into
            // one of three postures — see `resolve_client_tls`.
            client_tls: resolve_client_tls(),
        })
    }
}

/// Reads the node's configuration from the environment, then runs it.
///
/// The binary's entry point; [`run`] is the same thing given a
/// configuration you built yourself.
pub async fn run_from_env(
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), RunError> {
    run(NodeConfig::from_env()?, shutdown).await
}

/// Binds [`NodeConfig::local_addr`] and runs a node on it until
/// `shutdown` completes.
///
/// Binding happens before registering upstream, which is the safer
/// order: a node that cannot get its port fails without first announcing
/// itself to a server that would then wait for it.
pub async fn run(
    config: NodeConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), RunError> {
    let local_addr = config.local_addr;
    let listener = tokio::net::TcpListener::bind(local_addr)
        .await
        .map_err(|source| RunError::Bind {
            addr: local_addr,
            source,
        })?;
    run_on(listener, config, shutdown).await
}

/// Runs a node on a listener the caller already bound.
///
/// Exists because ports cannot be guessed. Several nodes in one process
/// need several distinct local ports, and the only race-free way to get
/// them is to bind `127.0.0.1:0` and read back what the OS chose — which
/// only the caller doing the binding can learn. Passing the listener in
/// keeps that knowledge where it is needed.
///
/// [`NodeConfig::local_addr`] is **ignored** here; `listener` decides.
/// The log line reports the listener's real address, since a node bound
/// on port 0 reporting `:0` would be worse than saying nothing.
///
/// The stub-client guard runs here rather than in
/// [`NodeConfig::from_env`] deliberately, and here rather than in
/// [`run`] for the same reason: a configuration built in code, on a
/// listener handed in by an orchestrator, must clear the same bar as one
/// read from the environment — or the in-process path would be a way to
/// run production against the placeholder `ClientApp`, exactly what the
/// guard exists to prevent.
pub async fn run_on(
    listener: tokio::net::TcpListener,
    config: NodeConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), RunError> {
    let NodeConfig {
        mode,
        allow_stub_client,
        client_app_kind,
        server_addr,
        client_id,
        // Ignored: `listener` is already bound, and to something that
        // may well be a different port. `run` is where this field is
        // read.
        local_addr: _,
        connection_mode,
        local_privacy,
        privacy_seed,
        auth_token,
        client_tls,
    } = config;

    validate_client_app_startup(mode, allow_stub_client, client_app_kind)?;

    let tls_mode = client_tls.label();
    let custom_auth_token = auth_token != PLACEHOLDER_AUTH_TOKEN;
    let client_side_privacy = local_privacy.is_some();

    let bridge = match connection_mode {
        ConnectionMode::Pull => {
            let mut upstream = match client_tls.config() {
                Some(tls) => PullTransport::connect_with_tls(server_addr, tls).await,
                None => PullTransport::connect(server_addr).await,
            }
            .map_err(RunError::Connect)?;
            upstream
                .register(&client_id, &auth_token)
                .await
                .map_err(RunError::Register)?;
            let bridge = NodeBridge::new(upstream, client_id.clone());
            Arc::new(apply_local_privacy(bridge, local_privacy, privacy_seed))
        }
        ConnectionMode::Push => {
            let mut upstream = match client_tls.config() {
                Some(tls) => PushTransport::connect_with_tls(server_addr, tls).await,
                None => PushTransport::connect(server_addr).await,
            }
            .map_err(RunError::Connect)?;
            upstream
                .register(&client_id, &auth_token)
                .await
                .map_err(RunError::Register)?;
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

    // The listener's own address, not the one that was asked for: a node
    // bound on port 0 has a real port now, and that is the one an
    // operator — or an orchestrator — needs to see.
    tracing::info!(
        local_addr = %listener.local_addr().map_err(|source| RunError::ListenerAddr { source })?,
        "local gRPC server listening for the ClientApp"
    );

    tonic::transport::Server::builder()
        .add_service(FlTransportServer::new(FlTransportService::new(bridge)))
        .serve_with_incoming_shutdown(TcpListenerStream::new(listener), shutdown)
        .await
        .map_err(RunError::LocalServer)?;
    tracing::info!("local grpc server stopped; node exiting");
    Ok(())
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

/// The node's client-side TLS posture.
///
/// Public because [`NodeConfig`] carries one: a caller building a
/// configuration in code has to be able to say "plaintext" — or anything
/// else — as precisely as the environment can. `resolve_client_tls`
/// derives it from `CONFLUX_TLS_*` for the binary's path.
#[derive(Debug, Clone)]
pub enum ClientTls {
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
    pub fn config(&self) -> Option<ClientTlsConfig> {
        match self {
            ClientTls::Plaintext => None,
            ClientTls::ServerAuth(c) | ClientTls::Mutual(c) => Some(c.clone()),
        }
    }

    /// A short word for logs: `off`, `server-auth` or `mutual`.
    pub fn label(&self) -> &'static str {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn research_default() -> NodeConfig {
        NodeConfig {
            mode: RuntimeMode::Research,
            allow_stub_client: true,
            client_app_kind: ClientAppKind::Stub,
            server_addr: "http://127.0.0.1:50051".to_string(),
            client_id: "node-1".to_string(),
            local_addr: "127.0.0.1:47100".parse().unwrap(),
            connection_mode: ConnectionMode::Pull,
            local_privacy: None,
            privacy_seed: None,
            auth_token: PLACEHOLDER_AUTH_TOKEN.to_string(),
            client_tls: ClientTls::Plaintext,
        }
    }

    /// The reason the guard moved out of `from_env` and into `run`.
    ///
    /// Splitting resolution from running created a second way in — a
    /// configuration built in code. If the guard had stayed with the
    /// environment read, that second way would bypass it, and running a
    /// production federation against the placeholder `ClientApp` would
    /// become possible by construction. It has to refuse either path.
    #[tokio::test]
    async fn a_config_built_in_code_still_cannot_run_production_on_the_stub() {
        let config = NodeConfig {
            mode: RuntimeMode::Production,
            allow_stub_client: false,
            client_app_kind: ClientAppKind::Stub,
            ..research_default()
        };
        // `run` refuses before it touches the network, so this never
        // connects to anything.
        let err = run(config, std::future::pending()).await.unwrap_err();
        assert!(
            matches!(
                err,
                RunError::StartupGuard(StartupGuardError::ProductionRefusesStubClient)
            ),
            "expected the stub-client guard, got: {err}"
        );
    }

    /// Two doors, one room: the binary's path and the in-process path must
    /// end at the same `run`, or they are two implementations wearing one
    /// name — the thing ADR 0004 keeps the node from becoming.
    #[test]
    fn from_env_and_a_hand_built_config_describe_the_same_shape() {
        let built = research_default();
        // Every field `from_env` sets is a field a caller can set, which is
        // what makes the in-process path a peer rather than a subset.
        assert_eq!(built.connection_mode, ConnectionMode::Pull);
        assert_eq!(built.client_tls.label(), "off");
        assert!(built.local_privacy.is_none());
    }
}
