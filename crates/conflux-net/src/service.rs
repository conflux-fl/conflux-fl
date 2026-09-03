//! Adapts a [`RoundDispatcher`] into a real tonic service implementing the
//! generated `fl_transport_server::FlTransport` trait.

use std::sync::Arc;

use conflux_proto::fl_transport_server::FlTransport;
use conflux_proto::{
    DeltaChunk, FetchTaskRequest, HeartbeatRequest, HeartbeatResponse, RegisterRequest,
    RegisterResponse, SubmitAck, SubscribeRequest, TaskResponse,
};
use tonic::{Request, Response, Status, Streaming};

use crate::dispatcher::{DispatchError, RoundDispatcher, TaskStream};
use crate::peer_identity::peer_cert_fingerprint;

/// Wraps `Arc<D>` in the shape tonic wants: a service struct implementing
/// the generated `FlTransport` trait, one method per RPC, each delegating
/// straight to the dispatcher.
pub struct FlTransportService<D> {
    dispatcher: Arc<D>,
    max_update_bytes: u64,
}

/// What [`FlTransportService::new`] uses when no explicit bound is set:
/// 256 MiB, matching `conflux-config`'s own `max_update_bytes` builtin.
///
/// Mirrored here rather than depending on `conflux-config` — this crate
/// sits beside it in the dependency graph, not above it — so the two must
/// be changed together. `conflux-config`'s value is the one a deployment
/// actually gets; this one only applies to a service constructed without
/// going through config at all (tests, and the `conflux-node` local hop).
pub const DEFAULT_MAX_UPDATE_BYTES: u64 = 256 * 1024 * 1024;

impl<D> FlTransportService<D> {
    /// Wraps a dispatcher in the shape tonic's generated server trait
    /// wants, bounding submitted updates at [`DEFAULT_MAX_UPDATE_BYTES`].
    pub fn new(dispatcher: Arc<D>) -> Self {
        Self {
            dispatcher,
            max_update_bytes: DEFAULT_MAX_UPDATE_BYTES,
        }
    }

    /// Sets the largest reassembled update this service will accept from
    /// one client, in bytes.
    ///
    /// Additive on purpose: `new` keeps its single-argument shape, so
    /// nothing that already constructs a service has to change to gain
    /// the bound — it gets the default instead of getting nothing.
    pub fn with_max_update_bytes(mut self, max_update_bytes: u64) -> Self {
        self.max_update_bytes = max_update_bytes;
        self
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
        // Collects the whole client stream before handing it to the
        // dispatcher — no incremental/backpressured delivery yet. A future
        // change could start aggregating before a client's last chunk
        // arrives, but nothing here needs that today.
        //
        // The running byte count is what keeps that "collect it all"
        // decision safe. gRPC's own limit is per *message*, so a client
        // sending an unbounded number of individually-legal chunks would
        // otherwise grow this `Vec` until the process dies. Checked as
        // each chunk arrives and before it is pushed, so the peak is one
        // chunk over the limit rather than however much the client felt
        // like sending.
        let mut stream = request.into_inner();
        let mut chunks: Vec<DeltaChunk> = Vec::new();
        let mut received_bytes: u64 = 0;
        while let Some(chunk) = stream.message().await? {
            // Every client-controlled byte in the chunk counts, not just
            // `data`. `control_variate` is also an arbitrary-length
            // client-supplied buffer — counting only
            // `data` would leave the ceiling intact while relocating the
            // flood one field to the left, which is not a bound at all.
            // Any future payload field belongs in this sum for the same
            // reason.
            let chunk_bytes = chunk.data.len() as u64
                + chunk
                    .control_variate
                    .as_ref()
                    .map_or(0, |cv| cv.len() as u64);
            received_bytes = received_bytes.saturating_add(chunk_bytes);
            if received_bytes > self.max_update_bytes {
                // Naming the client from the chunk in hand: this runs
                // before any identity check, because a check that ran
                // first would itself be what the flood hits. The id is
                // therefore claimed, not verified — which is why it is
                // documented as such on the error variant.
                let client_id = chunk.client_id;
                tracing::warn!(
                    client_id = %client_id,
                    limit_bytes = self.max_update_bytes,
                    received_bytes,
                    "rejecting oversized update; cutting the stream"
                );
                return Err(DispatchError::UpdateTooLarge {
                    client_id,
                    limit_bytes: self.max_update_bytes,
                    received_bytes,
                }
                .into());
            }
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
