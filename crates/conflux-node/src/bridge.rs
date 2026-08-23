//! `NodeBridge` — the local hop's `RoundDispatcher` implementation.
//!
//! Bridges the local loopback hop (Python `ClientApp` ↔ `conflux-node`) to
//! the real network hop (`conflux-node` ↔ `conflux-server`): the same
//! `.proto`, reused for both, per ADR 0004.

use std::time::Duration;

use conflux_net::{DispatchError, PullTransport, RoundDispatcher, TaskStream};
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use tokio::sync::Mutex;

/// `register`/`heartbeat` on the local hop are answered here without
/// touching the network — `conflux-node` already registered itself with
/// the real server at startup (spec §7); the local Python side isn't a
/// separate lifecycle entity the real server needs to track.
pub struct NodeBridge {
    upstream: Mutex<PullTransport>,
    node_client_id: String,
}

impl NodeBridge {
    pub fn new(upstream: PullTransport, node_client_id: String) -> Self {
        Self {
            upstream: Mutex::new(upstream),
            node_client_id,
        }
    }
}

const MAX_ATTEMPTS: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_millis(50);

#[async_trait::async_trait]
impl RoundDispatcher for NodeBridge {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        let mut upstream = self.upstream.lock().await;
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 1..=MAX_ATTEMPTS {
            match upstream.fetch_task(&self.node_client_id).await {
                Ok(task) => return Ok(task),
                Err(e) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(attempt, error = %e, ?backoff, "fetch_task attempt failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(e) => return Err(DispatchError::Other(e.to_string())),
            }
        }
        unreachable!("loop always returns by the last attempt")
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        // Push mode's node-side wiring is out of scope for Phase 6 (spec
        // §10 scopes the end-to-end test to pull mode) — `conflux-node`
        // only holds a `PullTransport` upstream.
        Err(DispatchError::Other(
            "push mode is not wired into conflux-node yet (Phase 6 scope: pull mode only)"
                .to_string(),
        ))
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        let mut upstream = self.upstream.lock().await;
        let mut backoff = INITIAL_BACKOFF;
        for attempt in 1..=MAX_ATTEMPTS {
            match upstream.submit_delta(chunks.clone()).await {
                Ok(ack) => return Ok(ack),
                Err(e) if attempt < MAX_ATTEMPTS => {
                    tracing::warn!(attempt, error = %e, ?backoff, "submit_delta attempt failed; retrying");
                    tokio::time::sleep(backoff).await;
                    backoff *= 2;
                }
                Err(e) => return Err(DispatchError::Other(e.to_string())),
            }
        }
        unreachable!("loop always returns by the last attempt")
    }

    async fn register(
        &self,
        _client_id: &str,
        _auth_token: &str,
        _peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        Ok(RegisterResponse {
            accepted: true,
            message: "conflux-node already registered with the real server".to_string(),
        })
    }

    async fn heartbeat(&self, _client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        Ok(HeartbeatResponse { acknowledged: true })
    }
}
