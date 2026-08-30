//! `AppState` answering `conflux-net`'s `RoundDispatcher` seam for real —
//! the piece Phase 3 explicitly left for whichever crate does the real
//! integration (spec: `docs/phases/phase-3-net.md`).

use std::sync::atomic::Ordering;

use conflux_buffer::BufferError;
use conflux_net::{DispatchError, RoundDispatcher, TaskStream};
use conflux_proto::{DeltaChunk, HeartbeatResponse, RegisterResponse, SubmitAck, TaskResponse};
use conflux_registry::{ClientId, NodeAllowlist, NodeIdentity, Registry, RegistryError};
use conflux_store::Store;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::AppState;

#[async_trait::async_trait]
impl RoundDispatcher for AppState {
    async fn fetch_task(&self, _client_id: &str) -> Result<TaskResponse, DispatchError> {
        let round = self.round.load(Ordering::SeqCst);
        let weights = self
            .store
            .load_latest_weights()
            .await
            .map_err(|e| DispatchError::Other(e.to_string()))?;
        Ok(TaskResponse {
            task_id: format!("round-{round}"),
            round,
            model_weights: conflux_proto::encode_weights(&weights),
        })
    }

    async fn subscribe_tasks(&self, _client_id: &str) -> Result<TaskStream, DispatchError> {
        let receiver = self.push_sender.subscribe();
        // A lagged subscriber (fell behind and missed broadcasts) just
        // skips the messages it missed rather than erroring the whole
        // stream — the next task it does see still has the current round.
        let stream = BroadcastStream::new(receiver).filter_map(|item| item.ok().map(Ok));
        Ok(Box::pin(stream))
    }

    async fn submit_delta(&self, chunks: Vec<DeltaChunk>) -> Result<SubmitAck, DispatchError> {
        let Some(first) = chunks.first() else {
            return Err(DispatchError::Other(
                "submit_delta called with zero chunks".to_string(),
            ));
        };
        let client_id = first.client_id.clone();
        let round = first.round;
        let num_samples = first.num_samples;

        let mut sorted = chunks;
        sorted.sort_by_key(|c| c.chunk_index);
        let mut data = Vec::new();
        for chunk in &sorted {
            data.extend_from_slice(&chunk.data);
        }

        let delta = conflux_proto::ClientDelta {
            client_id: client_id.clone(),
            round,
            weights: data,
            num_samples,
        };

        let buffer = self
            .current_buffer
            .lock()
            .expect("app state mutex poisoned")
            .clone();
        let Some(buffer) = buffer else {
            return Err(DispatchError::Other(
                "no round is currently accepting submissions".to_string(),
            ));
        };
        buffer.push(delta).map_err(|e| match e {
            BufferError::Closed => DispatchError::RoundClosed,
            other => DispatchError::Other(other.to_string()),
        })?;

        Ok(SubmitAck {
            accepted: true,
            message: format!("accepted delta from {client_id} for round {round}"),
        })
    }

    async fn register(
        &self,
        client_id: &str,
        auth_token: &str,
        peer_cert_fingerprint: Option<&str>,
    ) -> Result<RegisterResponse, DispatchError> {
        // Phase 16. Two independent gates, in this order:
        //
        //   1. Is this token genuine, and is it *yours*? (`auth = jwt`)
        //   2. Is this client on the allow-list? (`require_node_auth`)
        //
        // Neither subsumes the other, and a deployment can run either,
        // both, or neither. A valid JWT does not put a client on the
        // allow-list; being on the allow-list does not excuse an expired
        // token. They also fail as *different* `DispatchError` variants
        // — `Unauthenticated` vs. `NotAllowed` — because "your
        // credential is bad" and "your credential is fine but you aren't
        // invited" send an operator to two different places.
        crate::auth_enforcement::verify_jwt_if_required(
            self.config.mode,
            self.config.auth.value,
            self.jwt_key.as_ref(),
            auth_token,
            client_id,
        )
        .map_err(|e| DispatchError::Unauthenticated(e.to_string()))?;

        if self.config.require_node_auth.value {
            // The presented identity is whichever proof this connection
            // actually carries: an mTLS peer cert fingerprint if present,
            // else the request's shared token. Checked *before* touching
            // `conflux-registry` at all, so a rejected node never shows up
            // as a lifecycle registration attempt (spec: Phase 8c brief).
            let presented = match peer_cert_fingerprint {
                Some(fingerprint) => NodeIdentity::CertFingerprint(fingerprint.to_string()),
                None => NodeIdentity::SharedToken(auth_token.to_string()),
            };
            let allowed = self
                .node_allowlist
                .check(&ClientId(client_id.to_string()), &presented)
                .await
                .map_err(|e| DispatchError::Other(e.to_string()))?;
            if !allowed {
                return Err(DispatchError::NotAllowed(client_id.to_string()));
            }
        }

        match self
            .registry
            .register(ClientId(client_id.to_string()))
            .await
        {
            Ok(()) => Ok(RegisterResponse {
                accepted: true,
                message: "registered".to_string(),
            }),
            // A second registration from the same client is a retry, not
            // an error — punishing it would just make the client retry
            // harder.
            Err(RegistryError::AlreadyRegistered(_)) => Ok(RegisterResponse {
                accepted: true,
                message: "already registered".to_string(),
            }),
            Err(e) => Err(DispatchError::Other(e.to_string())),
        }
    }

    async fn heartbeat(&self, client_id: &str) -> Result<HeartbeatResponse, DispatchError> {
        match self
            .registry
            .heartbeat(&ClientId(client_id.to_string()))
            .await
        {
            Ok(()) => Ok(HeartbeatResponse { acknowledged: true }),
            Err(RegistryError::NotRegistered(id)) => {
                Err(DispatchError::UnknownClient(id.to_string()))
            }
            Err(e) => Err(DispatchError::Other(e.to_string())),
        }
    }
}
