//! A whole federation in one process.
//!
//! One `conflux-server`, N `conflux-node`s and — optionally — N Rust
//! `ClientApp`s, all running as tokio tasks in the process that called
//! [`Federation::start`] or [`run`]. Every hop is still real: nodes reach
//! the server over loopback gRPC, clients reach their node over the local
//! hop, updates are serialized, chunked and reassembled exactly as they
//! are across machines. **The only thing given up is process isolation.**
//!
//! That distinction matters enough to state plainly, because "in one
//! process" usually means a simulation. This is not one. A round run here
//! goes through the same buffer, quorum-or-timeout flush, server-side
//! privacy, reputation scoring, aggregation and checkpointing as a round
//! run across four machines, so a number it produces is a number about
//! the real pipeline. Nothing here is marked `simulated`, because nothing
//! here is simulated.
//!
//! # Why the ports are bound here
//!
//! N nodes need N distinct local ports, and every way of choosing them
//! without binding is a guess. A base port plus an index collides with
//! whatever else is on the machine; checking a port is free and *then*
//! binding it is a race rather than a check. Binding `127.0.0.1:0` lets
//! the OS assign, but only whoever binds can read back what it assigned —
//! so this crate binds every listener first and hands each one to
//! [`conflux_server::run_from_env_on`] or [`conflux_node::run_on`].
//!
//! Binding first buys something beyond distinct ports: **there is no
//! window in which the server is not yet reachable.** A node connecting
//! to a bound-but-not-yet-accepted listener completes its TCP connection
//! from the kernel's backlog and waits in the HTTP/2 handshake until the
//! server calls `accept`. The usual "poll the port until the server is
//! up" step has nothing left to wait for.
//!
//! [`Federation::start`] still waits for the server to answer `/health`,
//! for a different reason: to surface a *startup error* as a typed error
//! rather than as a node's transport failure thirty seconds later. A
//! mistyped aggregator name should be reported by the call that started
//! the server.
//!
//! # Shutdown
//!
//! One [`CancellationToken`] and one [`JoinSet`]. `conflux-server` uses a
//! `tokio::sync::watch` latch internally and is right to: it has three
//! fixed consumers. A federation is a tree of 2N+1 tasks, which is the
//! shape `CancellationToken` exists for — it is that same latch plus
//! hierarchy, so "stop the clients" and "stop everything" can be
//! different scopes without a second channel.
//!
//! # Node and client identity
//!
//! Node *i* and client *i* share one `client_id`, and this crate makes
//! that structural rather than a convention to remember. The local hop
//! absorbs a client's registration — it does not forward it, because the
//! node already registered upstream on the client's behalf — but
//! `submit_delta` passes chunks through unchanged, `client_id` and all.
//! Give the client a different id and the server receives an update from
//! a participant it never registered.

#![warn(missing_docs)]

pub mod demo;

use std::net::SocketAddr;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

// Re-exported so a caller can fill in a [`NodeTemplate`] without adding
// `conflux-node` and `conflux-privacy` as direct dependencies of their
// own. An orchestrator that makes you depend on the things it
// orchestrates has not saved you much.
pub use conflux_client::{ClientApp, RunConfig, TrainResult};
pub use conflux_node::{ClientAppKind, ClientTls, ConnectionMode, RuntimeMode};
pub use conflux_privacy::GaussianClippingPrivacy;

/// What every node in the federation shares.
///
/// A deliberate subset of [`conflux_node::NodeConfig`]: the three fields
/// a federation decides for itself — `server_addr`, `client_id` and
/// `local_addr` — are absent, because offering them would invite setting
/// values that are then silently overwritten. Everything else a node can
/// be told, it can be told here.
///
/// Kept in sync by the compiler rather than by discipline: the conversion
/// below builds a `NodeConfig` with a struct literal, so a new field on
/// `NodeConfig` fails this crate's build until it is decided here too.
#[derive(Debug, Clone)]
pub struct NodeTemplate {
    /// Behavioural mode; decides how strict the stub-client guard is.
    pub mode: RuntimeMode,
    /// Whether the placeholder Python `ClientApp` is permitted.
    pub allow_stub_client: bool,
    /// Which kind of `ClientApp` will connect to each local hop.
    ///
    /// [`run`] overrides this to [`ClientAppKind::Real`], because there
    /// the apps are Rust values supplied in code and therefore provably
    /// not `stub_client.py`. [`Federation::start`] does not, because it
    /// cannot know what will attach.
    pub client_app_kind: ClientAppKind,
    /// Push or pull.
    pub connection_mode: ConnectionMode,
    /// Optional client-side DP, applied before an update leaves a node.
    pub local_privacy: Option<GaussianClippingPrivacy>,
    /// Seed for that mechanism, when reproducibility matters.
    pub privacy_seed: Option<u64>,
    /// The credential every node presents at registration.
    pub auth_token: String,
    /// Client-side TLS posture for the upstream hop.
    pub client_tls: ClientTls,
}

impl Default for NodeTemplate {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::Research,
            allow_stub_client: false,
            client_app_kind: ClientAppKind::Real,
            connection_mode: ConnectionMode::Pull,
            local_privacy: None,
            privacy_seed: None,
            // Matches `conflux-node`'s own fallback. The server's auth
            // posture comes from its environment; on a research default
            // (`auth = none`) this is never checked.
            auth_token: "node-auth-token".to_string(),
            // Loopback inside one process. TLS here would encrypt a
            // conversation between two tasks on the same thread pool.
            client_tls: ClientTls::Plaintext,
        }
    }
}

/// How to run the federation.
#[derive(Debug, Clone)]
pub struct FederationConfig {
    /// How many nodes to run — and, in [`run`], how many clients.
    pub nodes: usize,
    /// Settings every node shares.
    pub node: NodeTemplate,
    /// Participant identities: `{prefix}-{i}`, for both node *i* and
    /// client *i*. See the module docs on why they must match.
    pub client_id_prefix: String,
    /// How long [`Federation::start`] waits for the server to answer
    /// `/health` before giving up on it.
    pub startup_timeout: Duration,
    /// How long [`Federation::shutdown`] lets tasks finish before
    /// aborting them.
    ///
    /// Short on purpose. The server's round loop checks for shutdown
    /// *between* rounds, and a round it has already opened waits for
    /// quorum or `round_timeout_secs` — five minutes by default. Once
    /// the clients have stopped, that quorum is never coming, so a long
    /// grace period buys a wait rather than a cleaner exit.
    pub shutdown_grace: Duration,
    /// The server's own configuration, or `None` to read it from the
    /// process environment as [`conflux_server::run_from_env_on`] does.
    ///
    /// `Some` is what lets a caller describe a whole federation as a
    /// value — an aggregator, a quorum and a model dimension chosen in
    /// code rather than exported first. `cflux fed run` builds one from
    /// a manifest; a sweep builds a different one per combination.
    pub server: Option<conflux_server::ServerConfig>,
    /// Rounds each client completes. [`run`] only.
    pub rounds: usize,
    /// How long a client waits before re-asking its node for a round
    /// that has not advanced. [`run`] only.
    ///
    /// Lower than [`RunConfig`]'s own default, deliberately.  That
    /// default is tuned for a client polling across a network hop while
    /// a round takes seconds; in one process a whole round can take
    /// under a millisecond, and a 200 ms poll would then be the entire
    /// cost of the run.
    pub client_poll_interval: Duration,
    /// Deadline for the whole run. [`run`] only — and not optional:
    /// `conflux_client::run` polls for a round it has not yet done and
    /// has no timeout of its own, so a server that stops advancing
    /// rounds would hang every client forever.
    pub timeout: Duration,
}

impl Default for FederationConfig {
    fn default() -> Self {
        Self {
            nodes: 2,
            node: NodeTemplate::default(),
            client_id_prefix: "client".to_string(),
            server: None,
            startup_timeout: Duration::from_secs(30),
            shutdown_grace: Duration::from_secs(2),
            rounds: 1,
            client_poll_interval: Duration::from_millis(5),
            timeout: Duration::from_secs(300),
        }
    }
}

/// Why a federation could not start, or did not finish.
#[derive(Debug, thiserror::Error)]
pub enum FederationError {
    /// A listener could not be bound. Rare with port 0 — it means the
    /// process is out of file descriptors or loopback is unavailable,
    /// not that something holds the port.
    #[error("cannot bind a loopback listener for the {what}: {source}")]
    Bind {
        /// Which listener: `"server"` or `"node 3"`.
        what: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// A bound listener would not report its own address. On port 0 that
    /// address is knowable only this way, so there is nothing to carry on
    /// with.
    #[error("the {what} listener would not report its own address: {source}")]
    ListenerAddr {
        /// Which listener.
        what: String,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The server refused to start, or stopped with an error.
    #[error("the server failed: {source}")]
    Server {
        /// The underlying error.
        source: conflux_server::ServeError,
    },
    /// The server never answered `/health`, and did not fail either.
    #[error(
        "the server did not answer /health on {addr} within {timeout:?}, and did not exit \
         with an error either — it is bound but not serving"
    )]
    ServerStartupTimeout {
        /// Where `/health` was being asked.
        addr: SocketAddr,
        /// How long it was given.
        timeout: Duration,
    },
    /// A node refused to start, or stopped with an error.
    #[error("node {index} ({client_id}) failed: {source}")]
    Node {
        /// Which node.
        index: usize,
        /// Its participant identity.
        client_id: String,
        /// The underlying error.
        source: conflux_node::RunError,
    },
    /// A client returned an error.
    #[error("client {index} ({client_id}) failed: {source}")]
    Client {
        /// Which client.
        index: usize,
        /// Its participant identity.
        client_id: String,
        /// The underlying error.
        source: conflux_client::ClientError,
    },
    /// The run hit its deadline with clients still going.
    ///
    /// Loud by design. A federation that quietly returned partial results
    /// is how a number from an unfinished run ends up being recorded as
    /// if it were a finished one.
    #[error(
        "the federation did not finish within {timeout:?}: {} of {total} client(s) were still \
         running — {}",
        unfinished.len(),
        unfinished
            .iter()
            .map(|c| format!("{} attempted {}/{} round(s)", c.client_id, c.rounds_attempted, c.rounds_requested))
            .collect::<Vec<_>>()
            .join("; ")
    )]
    Stalled {
        /// How long the run was given.
        timeout: Duration,
        /// How many clients there were in total.
        total: usize,
        /// The ones that had not finished, and how far each got.
        unfinished: Vec<ClientProgress>,
    },
    /// A task panicked rather than returning.
    #[error("the {what} task panicked: {source}")]
    Panic {
        /// Which task.
        what: String,
        /// The join error carrying the panic.
        source: tokio::task::JoinError,
    },
}

/// How far one client got, for a run that did not finish.
#[derive(Debug, Clone)]
pub struct ClientProgress {
    /// Which client.
    pub index: usize,
    /// Its participant identity.
    pub client_id: String,
    /// Rounds it began and reported an end for.
    ///
    /// *Attempted*, not completed: a submission the server rejected
    /// because the round closed on quorum while this client was still
    /// training counts here and does not count towards
    /// [`ClientOutcome::rounds_completed`]. Two different facts, so two
    /// different words.
    pub rounds_attempted: usize,
    /// What it was asked for.
    pub rounds_requested: usize,
}

/// What one client did.
#[derive(Debug, Clone)]
pub struct ClientOutcome {
    /// Which client.
    pub index: usize,
    /// Its participant identity.
    pub client_id: String,
    /// Submissions the node took, as counted by `conflux_client::run`.
    /// Whether the *server* accepted each one is a separate question,
    /// answered through `ClientApp::on_round_end`.
    pub rounds_completed: usize,
}

/// What a completed [`run`] did.
#[derive(Debug, Clone)]
pub struct FederationSummary {
    /// One entry per client, in index order.
    pub clients: Vec<ClientOutcome>,
    /// The server's gRPC address, in case the caller wants to report it.
    pub server_grpc_addr: SocketAddr,
    /// The server's HTTP admin address — `/rounds` is there, which is
    /// where a caller reads what actually happened.
    pub server_http_addr: SocketAddr,
}

/// A running server and its nodes, with every address already known.
///
/// Clients are not included: attach whatever you like to the addresses in
/// [`Self::node_urls`] — Rust `ClientApp`s in this process, Python
/// trainers in their own. [`run`] is the all-Rust case built on top of
/// this.
#[derive(Debug)]
pub struct Federation {
    grpc_addr: SocketAddr,
    http_addr: SocketAddr,
    node_addrs: Vec<SocketAddr>,
    client_ids: Vec<String>,
    cancel: CancellationToken,
    tasks: JoinSet<Component>,
    shutdown_grace: Duration,
    client_poll_interval: Duration,
}

/// Which task finished, and how. The `JoinSet` yields these in whatever
/// order tasks end, so each carries enough to name itself.
#[derive(Debug)]
enum Component {
    Server(Result<(), conflux_server::ServeError>),
    Node {
        index: usize,
        client_id: String,
        result: Result<(), conflux_node::RunError>,
    },
}

/// Binds one loopback listener on an OS-assigned port, and reads back
/// what the OS chose.
async fn bind_ephemeral(what: &str) -> Result<(TcpListener, SocketAddr), FederationError> {
    let listener =
        TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|source| FederationError::Bind {
                what: what.to_string(),
                source,
            })?;
    let addr = listener
        .local_addr()
        .map_err(|source| FederationError::ListenerAddr {
            what: what.to_string(),
            source,
        })?;
    Ok((listener, addr))
}

/// A bare HTTP/1.1 `GET /health`, returning true on a `200`.
///
/// Hand-written rather than pulled from an HTTP client: this crate needs
/// exactly one request, on loopback, to a response it reads four bytes
/// of. A dependency for that would cost more than it explains.
async fn health_ok(addr: SocketAddr) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let Ok(mut stream) = tokio::net::TcpStream::connect(addr).await else {
        return false;
    };
    let request = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).await.is_err() {
        return false;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).await.is_err() {
        return false;
    }
    response.starts_with("HTTP/1.1 200")
}

impl Federation {
    /// Binds every listener, starts the server and `config.nodes` nodes,
    /// and returns once the server is serving.
    ///
    /// The server's own configuration still comes from the process
    /// environment. That is not an oversight: one process is one
    /// experiment, so one environment describes the server exactly. Only
    /// the *ports* have to come from outside, and only because they
    /// cannot be guessed.
    ///
    /// ```no_run
    /// use conflux_federation::{Federation, FederationConfig};
    ///
    /// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
    /// let fed = Federation::start(FederationConfig {
    ///     nodes: 4,
    ///     ..Default::default()
    /// })
    /// .await?;
    ///
    /// // Attach whatever you like — Rust apps here, Python trainers in
    /// // their own processes. A client must present the same identity
    /// // as the node it attaches to.
    /// for (i, url) in fed.node_urls().iter().enumerate() {
    ///     println!("node {i}: {url} — client id {}", fed.client_id(i));
    /// }
    ///
    /// fed.shutdown().await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn start(config: FederationConfig) -> Result<Self, FederationError> {
        let (grpc_listener, grpc_addr) = bind_ephemeral("server's gRPC transport").await?;
        let (http_listener, http_addr) = bind_ephemeral("server's admin API").await?;

        // Every node's listener, bound before anything starts. From here
        // on the addresses are facts, not intentions.
        let mut node_listeners = Vec::with_capacity(config.nodes);
        let mut node_addrs = Vec::with_capacity(config.nodes);
        for i in 0..config.nodes {
            let (listener, addr) = bind_ephemeral(&format!("node {i}")).await?;
            node_listeners.push(listener);
            node_addrs.push(addr);
        }

        let client_ids: Vec<String> = (0..config.nodes)
            .map(|i| format!("{}-{i}", config.client_id_prefix))
            .collect();

        let cancel = CancellationToken::new();
        let mut tasks = JoinSet::new();

        let server_cancel = cancel.clone();
        let server_config = config.server.clone();
        tasks.spawn(async move {
            let listeners = conflux_server::ServerListeners {
                grpc: grpc_listener,
                http: http_listener,
            };
            let shutdown = server_cancel.cancelled_owned();
            Component::Server(match server_config {
                Some(config) => conflux_server::run_on(listeners, config, shutdown).await,
                None => conflux_server::run_from_env_on(listeners, shutdown).await,
            })
        });

        // Wait for the server before starting nodes. Not because a node
        // would fail otherwise — it would connect into the backlog and
        // wait — but so that a server which refuses its configuration is
        // reported by *this* call, with its own error, instead of
        // surfacing later as N identical transport failures.
        Self::await_server(&mut tasks, http_addr, config.startup_timeout, &cancel).await?;

        let grpc_url = format!("http://{grpc_addr}");
        for (i, listener) in node_listeners.into_iter().enumerate() {
            let node_config = conflux_node::NodeConfig {
                mode: config.node.mode,
                allow_stub_client: config.node.allow_stub_client,
                client_app_kind: config.node.client_app_kind,
                server_addr: grpc_url.clone(),
                client_id: client_ids[i].clone(),
                // Ignored by `run_on`; the listener above decides. Set to
                // the address it actually has so a `Debug` of this config
                // does not read as a lie.
                local_addr: node_addrs[i],
                connection_mode: config.node.connection_mode,
                local_privacy: config.node.local_privacy,
                privacy_seed: config.node.privacy_seed,
                auth_token: config.node.auth_token.clone(),
                client_tls: config.node.client_tls.clone(),
            };
            let node_cancel = cancel.clone();
            let client_id = client_ids[i].clone();
            tasks.spawn(async move {
                let result =
                    conflux_node::run_on(listener, node_config, node_cancel.cancelled_owned())
                        .await;
                Component::Node {
                    index: i,
                    client_id,
                    result,
                }
            });
        }

        tracing::info!(
            nodes = config.nodes,
            %grpc_addr,
            %http_addr,
            "federation up: one server and {} node(s) in this process",
            config.nodes
        );

        Ok(Self {
            grpc_addr,
            http_addr,
            node_addrs,
            client_ids,
            cancel,
            tasks,
            shutdown_grace: config.shutdown_grace,
            client_poll_interval: config.client_poll_interval,
        })
    }

    /// Races the server's startup against its own failure.
    ///
    /// Three outcomes, and all three are reported rather than waited
    /// through: it answers `/health` (ready), its task returns (it
    /// refused its configuration, and that error is the useful one), or
    /// neither happens in time.
    async fn await_server(
        tasks: &mut JoinSet<Component>,
        http_addr: SocketAddr,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<(), FederationError> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            tokio::select! {
                // Biased so an already-failed server is reported as its
                // own error rather than as a timeout, even if both are
                // ready in the same poll.
                biased;

                joined = tasks.join_next() => {
                    // The server is the only task running at this point,
                    // so anything yielded here is the server exiting.
                    return match joined {
                        Some(Ok(Component::Server(Err(source)))) =>
                            Err(FederationError::Server { source }),
                        Some(Ok(Component::Server(Ok(())))) =>
                            Err(FederationError::ServerStartupTimeout { addr: http_addr, timeout }),
                        Some(Err(source)) => Err(FederationError::Panic {
                            what: "server".to_string(),
                            source,
                        }),
                        // Unreachable in practice; treated as a failure
                        // rather than asserted, because a panic here
                        // would be a worse way to say the same thing.
                        Some(Ok(Component::Node { index, client_id, result })) => match result {
                            Err(source) => Err(FederationError::Node { index, client_id, source }),
                            Ok(()) => Err(FederationError::ServerStartupTimeout { addr: http_addr, timeout }),
                        },
                        None => Err(FederationError::ServerStartupTimeout { addr: http_addr, timeout }),
                    };
                }

                _ = tokio::time::sleep_until(deadline) => {
                    cancel.cancel();
                    return Err(FederationError::ServerStartupTimeout { addr: http_addr, timeout });
                }

                ready = health_ok(http_addr) => {
                    if ready {
                        return Ok(());
                    }
                    // Bound but not yet accepting. Back off briefly
                    // rather than spinning on connect.
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
            }
        }
    }

    /// The gRPC address nodes and external clients connect to.
    pub fn server_grpc_addr(&self) -> SocketAddr {
        self.grpc_addr
    }

    /// The same address as a URL, which is the form
    /// [`conflux_node::NodeConfig::server_addr`] wants.
    pub fn server_grpc_url(&self) -> String {
        format!("http://{}", self.grpc_addr)
    }

    /// The HTTP admin address — `/health`, `/round/status`, `/rounds`.
    pub fn server_http_addr(&self) -> SocketAddr {
        self.http_addr
    }

    /// Every node's local hop, in index order.
    pub fn node_addrs(&self) -> &[SocketAddr] {
        &self.node_addrs
    }

    /// Every node's local hop as a URL, which is the form
    /// [`conflux_client::RunConfig::address`] wants.
    pub fn node_urls(&self) -> Vec<String> {
        self.node_addrs
            .iter()
            .map(|addr| format!("http://{addr}"))
            .collect()
    }

    /// The participant identity of node *i*.
    ///
    /// A client attaching to node *i* must present this same id — see the
    /// module docs.
    pub fn client_id(&self, index: usize) -> &str {
        &self.client_ids[index]
    }

    /// A ready-made [`RunConfig`] for the client of node *i*: the right
    /// address and the right identity, which are the two things easy to
    /// get wrong by hand.
    pub fn client_config(&self, index: usize, rounds: usize) -> RunConfig {
        RunConfig {
            address: format!("http://{}", self.node_addrs[index]),
            client_id: self.client_ids[index].clone(),
            rounds,
            poll_interval: self.client_poll_interval,
            ..RunConfig::default()
        }
    }

    /// A token that fires when this federation is shutting down, so a
    /// caller's own tasks can stop with it.
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Stops the server and every node, and reports the first component
    /// that failed.
    ///
    /// Every task is drained even after a failure is found: leaving one
    /// running would leave a listener open in a process that believes it
    /// shut down.
    pub async fn shutdown(mut self) -> Result<(), FederationError> {
        self.cancel.cancel();

        let drain = async {
            let mut first_error = None;
            while let Some(joined) = self.tasks.join_next().await {
                let error = match joined {
                    Ok(Component::Server(Err(source))) => Some(FederationError::Server { source }),
                    Ok(Component::Node {
                        index,
                        client_id,
                        result: Err(source),
                    }) => Some(FederationError::Node {
                        index,
                        client_id,
                        source,
                    }),
                    Ok(_) => None,
                    Err(source) if source.is_cancelled() => None,
                    Err(source) => Some(FederationError::Panic {
                        what: "federation".to_string(),
                        source,
                    }),
                };
                if let Some(error) = error {
                    tracing::warn!(%error, "a federation component stopped with an error");
                    first_error.get_or_insert(error);
                }
            }
            first_error
        };

        match tokio::time::timeout(self.shutdown_grace, drain).await {
            Ok(None) => Ok(()),
            Ok(Some(error)) => Err(error),
            Err(_) => {
                // The grace period is over. Aborting is the honest end:
                // the alternative is a `shutdown` that never returns.
                tracing::info!(
                    grace = ?self.shutdown_grace,
                    "aborting components that did not stop within the grace period — \
                     ordinary when the clients have finished, since the round the server \
                     is in is waiting for a quorum that will not arrive"
                );
                self.tasks.abort_all();
                Ok(())
            }
        }
    }
}

/// Wraps a caller's app to count the rounds it begins and ends.
///
/// Needed because `conflux_client::run` reports its count only by
/// returning, and a client that never finishes never returns one — which
/// is exactly the case [`FederationError::Stalled`] has to describe.
struct Counted<A> {
    inner: A,
    rounds: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl<A: ClientApp> ClientApp for Counted<A> {
    fn train(&mut self, weights: &[f32], round: u64) -> TrainResult {
        self.inner.train(weights, round)
    }
    fn on_round_start(&mut self, round: u64) {
        self.inner.on_round_start(round);
    }
    fn on_control_variate(&mut self, c: &[f32]) {
        self.inner.on_control_variate(c);
    }
    fn on_round_end(&mut self, round: u64, accepted: bool) {
        self.rounds
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.inner.on_round_end(round, accepted);
    }
}

/// Runs a complete federation — server, nodes and Rust clients — and
/// returns when every client has finished its rounds.
///
/// `make_app` is called once per client with its index, so each can take
/// its own data shard. The apps are Rust values supplied in code, so
/// `client_app_kind` is forced to [`ClientAppKind::Real`]: the stub guard
/// exists to keep `stub_client.py` out of production, and a value
/// constructed here is provably not that file.
///
/// Fails loudly on a stall. `config.timeout` is a real deadline, and
/// exceeding it is [`FederationError::Stalled`] naming every client that
/// did not finish and how far it got — not a summary of partial results
/// that reads like a completed run.
///
/// ```no_run
/// use conflux_federation::{ClientApp, FederationConfig, TrainResult};
///
/// struct MyApp {
///     shard: usize,
/// }
///
/// impl ClientApp for MyApp {
///     fn train(&mut self, weights: &[f32], _round: u64) -> TrainResult {
///         TrainResult::new(weights.to_vec(), 100)
///     }
/// }
///
/// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// let summary = conflux_federation::run(
///     FederationConfig {
///         nodes: 4,
///         rounds: 10,
///         ..Default::default()
///     },
///     |i| MyApp { shard: i },
/// )
/// .await?;
///
/// for outcome in &summary.clients {
///     println!("{} completed {} round(s)", outcome.client_id, outcome.rounds_completed);
/// }
/// # Ok(())
/// # }
/// ```
pub async fn run<A, F>(
    mut config: FederationConfig,
    mut make_app: F,
) -> Result<FederationSummary, FederationError>
where
    F: FnMut(usize) -> A,
    A: ClientApp + Send + 'static,
{
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let rounds = config.rounds;
    let timeout = config.timeout;
    let total = config.nodes;

    config.node.client_app_kind = ClientAppKind::Real;

    let federation = Federation::start(config).await?;

    // One counter per client, read only if the run stalls.
    let counters: Vec<Arc<AtomicUsize>> =
        (0..total).map(|_| Arc::new(AtomicUsize::new(0))).collect();

    let mut clients = JoinSet::new();
    for (index, counter) in counters.iter().enumerate() {
        let run_config = federation.client_config(index, rounds);
        let client_id = federation.client_id(index).to_string();
        let mut app = Counted {
            inner: make_app(index),
            rounds: Arc::clone(counter),
        };
        clients.spawn(async move {
            let result = conflux_client::run(&mut app, run_config).await;
            (index, client_id, result)
        });
    }

    let collect = async {
        let mut outcomes = Vec::with_capacity(total);
        while let Some(joined) = clients.join_next().await {
            match joined {
                Ok((index, client_id, Ok(rounds_completed))) => outcomes.push(ClientOutcome {
                    index,
                    client_id,
                    rounds_completed,
                }),
                Ok((index, client_id, Err(source))) => {
                    return Err(FederationError::Client {
                        index,
                        client_id,
                        source,
                    });
                }
                Err(source) => {
                    return Err(FederationError::Panic {
                        what: "a client".to_string(),
                        source,
                    });
                }
            }
        }
        outcomes.sort_by_key(|o| o.index);
        Ok(outcomes)
    };

    let grpc_addr = federation.server_grpc_addr();
    let http_addr = federation.server_http_addr();

    let result = match tokio::time::timeout(timeout, collect).await {
        Ok(Ok(clients)) => Ok(FederationSummary {
            clients,
            server_grpc_addr: grpc_addr,
            server_http_addr: http_addr,
        }),
        Ok(Err(error)) => Err(error),
        Err(_) => {
            // Which clients were still going, and how far each got. The
            // finished ones have already dropped out of the JoinSet, so
            // anything still in it is unfinished.
            let unfinished: Vec<ClientProgress> = (0..total)
                .map(|index| ClientProgress {
                    index,
                    client_id: federation.client_id(index).to_string(),
                    rounds_attempted: counters[index].load(Ordering::Relaxed),
                    rounds_requested: rounds,
                })
                .filter(|p| p.rounds_attempted < rounds)
                .collect();
            Err(FederationError::Stalled {
                timeout,
                total,
                unfinished,
            })
        }
    };

    // Down in both directions: a failed run must not leave a server and N
    // nodes holding listeners in the caller's process.
    clients.abort_all();
    let shutdown = federation.shutdown().await;

    // The run's own error outranks a shutdown error, which is almost
    // always a consequence of it.
    match (result, shutdown) {
        (Ok(summary), Ok(())) => Ok(summary),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), _) => Err(error),
    }
}
