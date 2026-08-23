//! Client-side transport wrappers around the generated
//! `FlTransportClient<Channel>` — spec §10's `PullTransport`/`PushTransport`
//! naming. `conflux-node` (Phase 6) will hold whichever one matches its
//! resolved `connection_mode`.

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

/// Pull mode (spec §3: `cross_device`, `crowdsource`, `edge`) — the client
/// asks for its next task.
pub struct PullTransport {
    client: FlTransportClient<Channel>,
}

impl PullTransport {
    pub async fn connect(addr: impl Into<String>) -> Result<Self, TransportError> {
        let client = FlTransportClient::connect(addr.into()).await?;
        Ok(Self { client })
    }

    /// Spec §3: `cross_silo` is push + mTLS, but nothing stops a pull-mode
    /// deployment from wanting mTLS too — this isn't topology-gated.
    pub async fn connect_with_tls(
        addr: impl Into<String>,
        tls: ClientTlsConfig,
    ) -> Result<Self, TransportError> {
        let channel = connect_channel(addr, Some(tls)).await?;
        Ok(Self {
            client: FlTransportClient::new(channel),
        })
    }

    pub async fn fetch_task(&mut self, client_id: &str) -> Result<TaskResponse, TransportError> {
        let response = self
            .client
            .fetch_task(FetchTaskRequest {
                client_id: client_id.to_string(),
            })
            .await?;
        Ok(response.into_inner())
    }

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

    pub async fn submit_delta(
        &mut self,
        chunks: Vec<DeltaChunk>,
    ) -> Result<SubmitAck, TransportError> {
        let response = self.client.submit_delta(tokio_stream::iter(chunks)).await?;
        Ok(response.into_inner())
    }
}

/// Push mode (spec §3: `cross_silo`) — the server streams tasks to a
/// subscribed client.
pub struct PushTransport {
    client: FlTransportClient<Channel>,
}

impl PushTransport {
    pub async fn connect(addr: impl Into<String>) -> Result<Self, TransportError> {
        let client = FlTransportClient::connect(addr.into()).await?;
        Ok(Self { client })
    }

    /// Spec §3: `cross_silo` push mode is where mTLS actually applies.
    pub async fn connect_with_tls(
        addr: impl Into<String>,
        tls: ClientTlsConfig,
    ) -> Result<Self, TransportError> {
        let channel = connect_channel(addr, Some(tls)).await?;
        Ok(Self {
            client: FlTransportClient::new(channel),
        })
    }

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

    pub async fn submit_delta(
        &mut self,
        chunks: Vec<DeltaChunk>,
    ) -> Result<SubmitAck, TransportError> {
        let response = self.client.submit_delta(tokio_stream::iter(chunks)).await?;
        Ok(response.into_inner())
    }
}
