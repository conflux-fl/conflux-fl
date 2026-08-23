//! Adapts a [`RoundDispatcher`] into a real tonic service implementing the
//! generated `fl_transport_server::FlTransport` trait.

use std::sync::Arc;

use conflux_proto::fl_transport_server::FlTransport;
use conflux_proto::{
    DeltaChunk, FetchTaskRequest, HeartbeatRequest, HeartbeatResponse, RegisterRequest,
    RegisterResponse, SubmitAck, SubscribeRequest, TaskResponse,
};
use tonic::{Request, Response, Status, Streaming};

use crate::dispatcher::{RoundDispatcher, TaskStream};
use crate::peer_identity::peer_cert_fingerprint;

/// Wraps `Arc<D>` in the shape tonic wants: a service struct implementing
/// the generated `FlTransport` trait, one method per RPC, each delegating
/// straight to the dispatcher.
pub struct FlTransportService<D> {
    dispatcher: Arc<D>,
}

impl<D> FlTransportService<D> {
    pub fn new(dispatcher: Arc<D>) -> Self {
        Self { dispatcher }
    }
}

#[async_trait::async_trait]
impl<D: RoundDispatcher> FlTransport for FlTransportService<D> {
    type SubscribeTasksStream = TaskStream;

    async fn fetch_task(
        &self,
        request: Request<FetchTaskRequest>,
    ) -> Result<Response<TaskResponse>, Status> {
        let req = request.into_inner();
        let task = self.dispatcher.fetch_task(&req.client_id).await?;
        Ok(Response::new(task))
    }

    async fn subscribe_tasks(
        &self,
        request: Request<SubscribeRequest>,
    ) -> Result<Response<Self::SubscribeTasksStream>, Status> {
        let req = request.into_inner();
        let stream = self.dispatcher.subscribe_tasks(&req.client_id).await?;
        Ok(Response::new(stream))
    }

    async fn submit_delta(
        &self,
        request: Request<Streaming<DeltaChunk>>,
    ) -> Result<Response<SubmitAck>, Status> {
        // Phase 3 collects the whole stream before handing it to the
        // dispatcher — no incremental/backpressured delivery yet. Revisit
        // if a future phase needs to start aggregating before a client's
        // last chunk arrives.
        let mut stream = request.into_inner();
        let mut chunks = Vec::new();
        while let Some(chunk) = stream.message().await? {
            chunks.push(chunk);
        }
        let ack = self.dispatcher.submit_delta(chunks).await?;
        Ok(Response::new(ack))
    }

    async fn register(
        &self,
        request: Request<RegisterRequest>,
    ) -> Result<Response<RegisterResponse>, Status> {
        // Extracted before `into_inner()` — the peer cert lives in the
        // request's extensions, not its body.
        let fingerprint = peer_cert_fingerprint(&request);
        let req = request.into_inner();
        let resp = self
            .dispatcher
            .register(&req.client_id, &req.auth_token, fingerprint.as_deref())
            .await?;
        Ok(Response::new(resp))
    }

    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let req = request.into_inner();
        let resp = self.dispatcher.heartbeat(&req.client_id).await?;
        Ok(Response::new(resp))
    }
}
