//! Client-side transport wrappers around the generated
//! `FlTransportClient<Channel>`: [`PullTransport`] for pull-mode
//! deployments, [`PushTransport`] for push-mode ones. `conflux-node` picks
//! whichever one matches its resolved `connection_mode` at startup and
//! holds onto it for the life of the process.

use conflux_proto::fl_transport_client::FlTransportClient;
use conflux_proto::{
    DeltaChunk, FetchTaskRequest, HeartbeatRequest, HeartbeatResponse, RegisterRequest,
    RegisterResponse, SubmitAck, SubscribeRequest, TaskResponse,
};
use tonic::Streaming;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::TransportError;

async fn connect_channel(
    addr: impl Into<String>,
    tls: Option<ClientTlsConfig>,
) -> Result<Channel, TransportError> {
    let mut endpoint = Endpoint::from_shared(addr.into())?;
    if let Some(tls) = tls {
        endpoint = endpoint.tls_config(tls)?;
    }
    Ok(endpoint.connect().await?)
}

/// Pull mode (the default for `cross_device`, `crowdsource`, and `edge`
/// topologies — many, intermittently-connected participants that check in
/// on their own schedule) — the client asks for its next task.
pub struct PullTransport {
    client: FlTransportClient<Channel>,
}

impl PullTransport {
    /// Connects without TLS — a plaintext hop, such as the local loopback
    /// connection or a development deployment.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, TransportError> {
        let client = FlTransportClient::connect(addr.into()).await?;
        Ok(Self { client })
    }

    /// `cross_silo` defaults to push + mTLS, but nothing here is
    /// topology-gated — a pull-mode deployment can use mTLS too, it just
    /// isn't `cross_silo`'s default posture.
    pub async fn connect_with_tls(
        addr: impl Into<String>,
        tls: ClientTlsConfig,
    ) -> Result<Self, TransportError> {
        let channel = connect_channel(addr, Some(tls)).await?;
        Ok(Self {
            client: FlTransportClient::new(channel),
        })
    }

    /// Asks the server for this client's next task.
    pub async fn fetch_task(&mut self, client_id: &str) -> Result<TaskResponse, TransportError> {
        let response = self
            .client
            .fetch_task(FetchTaskRequest {
                client_id: client_id.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

    /// Registers this client. `auth_token` is a signed JWT when the
    /// deployment's auth mode is `jwt`, otherwise a pre-shared token.
    pub async fn register(
        &mut self,
        client_id: &str,
        auth_token: &str,
    ) -> Result<RegisterResponse, TransportError> {
        let response = self
            .client
            .register(RegisterRequest {
                client_id: client_id.to_string(),
                auth_token: auth_token.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

    /// Tells the server this client is still alive, resetting its
    /// eviction clock.
    pub async fn heartbeat(
        &mut self,
        client_id: &str,
    ) -> Result<HeartbeatResponse, TransportError> {
        let response = self
            .client
            .heartbeat(HeartbeatRequest {
                client_id: client_id.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

    /// Streams a trained update back. Chunks are sent in the order given;
    /// the server sorts by `chunk_index` before reassembling, so that
    /// order doesn't matter.
    pub async fn submit_delta(
        &mut self,
        chunks: Vec<DeltaChunk>,
    ) -> Result<SubmitAck, TransportError> {
        let response = self.client.submit_delta(tokio_stream::iter(chunks)).await?;
        Ok(response.into_inner())
    }
}

/// Push mode (the default for `cross_silo` — few, trusted, always-reachable
/// institutional participants that can hold an open connection) — the
/// server streams tasks to a subscribed client.
///
/// `Clone` matters here in a way it doesn't for [`PullTransport`]. Push
/// mode holds one long-lived subscription open while ordinary
/// request/response calls (`submit_delta`, `heartbeat`) still have to go
/// out on the same connection. Sharing a single transport behind one lock
/// would make those calls wait on the subscription's own re-subscribe
/// attempts; cloning instead gives each concurrent user its own handle.
/// That's cheap and safe because the underlying `Channel` is itself a
/// cheaply-cloneable handle to one HTTP/2 connection, which multiplexes
/// concurrent streams by design — a clone is another stream on the same
/// connection, not another connection.
#[derive(Clone)]
pub struct PushTransport {
    client: FlTransportClient<Channel>,
}

impl PushTransport {
    /// Connects without TLS. Push mode's default posture is mTLS — see
    /// [`PushTransport::connect_with_tls`].
    pub async fn connect(addr: impl Into<String>) -> Result<Self, TransportError> {
        let client = FlTransportClient::connect(addr.into()).await?;
        Ok(Self { client })
    }

    /// `cross_silo` push mode is where mTLS is used by default, but this
    /// constructor isn't restricted to that topology.
    pub async fn connect_with_tls(
        addr: impl Into<String>,
        tls: ClientTlsConfig,
    ) -> Result<Self, TransportError> {
        let channel = connect_channel(addr, Some(tls)).await?;
        Ok(Self {
            client: FlTransportClient::new(channel),
        })
    }

    /// Opens a long-lived subscription. The returned stream yields a
    /// `TaskResponse` each time a round opens, until the server closes it.
    pub async fn subscribe_tasks(
        &mut self,
        client_id: &str,
    ) -> Result<Streaming<TaskResponse>, TransportError> {
        let response = self
            .client
            .subscribe_tasks(SubscribeRequest {
                client_id: client_id.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

    /// Registers this client over the push connection.
    pub async fn register(
        &mut self,
        client_id: &str,
        auth_token: &str,
    ) -> Result<RegisterResponse, TransportError> {
        let response = self
            .client
            .register(RegisterRequest {
                client_id: client_id.to_string(),
                auth_token: auth_token.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

    /// Tells the server this client is still alive.
    pub async fn heartbeat(
        &mut self,
        client_id: &str,
    ) -> Result<HeartbeatResponse, TransportError> {
        let response = self
            .client
            .heartbeat(HeartbeatRequest {
                client_id: client_id.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

    /// Streams a trained update back, exactly as pull mode does — only
    /// task *acquisition* differs between the two modes.
    pub async fn submit_delta(
        &mut self,
        chunks: Vec<DeltaChunk>,
    ) -> Result<SubmitAck, TransportError> {
        let response = self.client.submit_delta(tokio_stream::iter(chunks)).await?;
        Ok(response.into_inner())
    }
}
