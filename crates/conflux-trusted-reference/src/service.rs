//! The gRPC server half of the sidecar hop — [`TrustedReferenceService`]
//! wraps any [`TrustedModel`] and answers `conflux-server`'s calls.
//!
//! Kept separate from the model itself so a deployer implementing
//! [`TrustedModel`] never writes gRPC code, and so this file has exactly
//! one job: translating flat `f32` buffers to and from a model that knows
//! what they mean. The server on the other end still does not (ADR 0004).

use std::sync::Arc;

use conflux_proto::trusted_reference_server::{TrustedReference, TrustedReferenceServer};
use conflux_proto::{
    ClientScore, DescribeRequest, DescribeResponse, ReferenceRequest, ReferenceUpdate,
    ScoreRequest, ScoreResponse,
};
use tonic::{Request, Response, Status};

use crate::TrustedModel;

/// Decodes a little-endian packed `f32` buffer — the same encoding
/// `conflux-proto::decode_weights` defines.
///
/// Reimplemented here rather than imported so this crate does not need a
/// dependency edge for four lines, and because the failure needs to
/// become a `Status` either way.
fn decode(bytes: &[u8], field: &str) -> Result<Vec<f32>, Status> {
    if bytes.len() % 4 != 0 {
        return Err(Status::invalid_argument(format!(
            "{field} is {} bytes, which is not a whole number of f32s",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

fn encode(weights: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(weights.len() * 4);
    for w in weights {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    bytes
}

/// Serves a [`TrustedModel`] over the `TrustedReference` gRPC service.
pub struct TrustedReferenceService<M> {
    model: Arc<M>,
}

impl<M: TrustedModel> TrustedReferenceService<M> {
    /// Wraps `model`.
    pub fn new(model: M) -> Self {
        Self {
            model: Arc::new(model),
        }
    }

    /// Wraps an already-shared model, for a caller that also holds it.
    pub fn from_arc(model: Arc<M>) -> Self {
        Self { model }
    }
}

#[tonic::async_trait]
impl<M: TrustedModel + 'static> TrustedReference for TrustedReferenceService<M> {
    async fn get_reference_update(
        &self,
        request: Request<ReferenceRequest>,
    ) -> Result<Response<ReferenceUpdate>, Status> {
        let req = request.into_inner();

        if !self.model.supports_reference_update() {
            // `unimplemented` rather than `internal`: this sidecar is
            // working correctly and simply cannot serve FLTrust. The
            // server's startup `Describe` call should have caught this,
            // so reaching here means a misconfiguration that outlived
            // startup — worth an unambiguous code.
            return Err(Status::unimplemented(
                "this sidecar does not implement reference updates; it cannot serve fltrust",
            ));
        }

        let global = decode(&req.global_weights, "global_weights")?;
        let reference = self.model.train_reference(&global);

        // The one invariant the service enforces on its own model: a
        // reference of the wrong length is not a weaker reference, it is
        // an unusable one, and letting it reach the server would push the
        // failure into an aggregator that has less context to explain it.
        if reference.len() != global.len() {
            return Err(Status::internal(format!(
                "trusted model returned {} weights for a {}-weight model",
                reference.len(),
                global.len()
            )));
        }
        if let Some(index) = reference.iter().position(|w| !w.is_finite()) {
            return Err(Status::internal(format!(
                "trusted model returned a non-finite reference at index {index}; \
                 scoring honest clients against it would silently break the defense"
            )));
        }

        tracing::debug!(
            round = req.round,
            dim = reference.len(),
            "served a trusted reference update"
        );

        Ok(Response::new(ReferenceUpdate {
            round: req.round,
            weights: encode(&reference),
            local_steps: None,
        }))
    }

    async fn score_updates(
        &self,
        request: Request<ScoreRequest>,
    ) -> Result<Response<ScoreResponse>, Status> {
        let req = request.into_inner();

        if !self.model.supports_scoring() {
            return Err(Status::unimplemented(
                "this sidecar does not implement scoring; it cannot serve zeno",
            ));
        }

        let global = decode(&req.global_weights, "global_weights")?;

        let mut scores = Vec::with_capacity(req.candidates.len());
        for candidate in &req.candidates {
            let weights = decode(&candidate.weights, "candidate weights")?;
            let score = self.model.score(&global, &weights);
            // A `NaN` score is the model saying it has no opinion. Omit
            // the entry rather than reporting a number, because every
            // comparison against `NaN` is false — a caller that ranked by
            // it would silently treat "unscoreable" as "not worse than
            // anything", which is the most dangerous possible reading.
            if score.is_nan() {
                tracing::debug!(
                    client_id = %candidate.client_id,
                    "no score available; omitting rather than reporting NaN"
                );
                continue;
            }
            scores.push(ClientScore {
                client_id: candidate.client_id.clone(),
                score,
            });
        }

        tracing::debug!(
            round = req.round,
            scored = scores.len(),
            requested = req.candidates.len(),
            "served client scores"
        );

        Ok(Response::new(ScoreResponse {
            round: req.round,
            scores,
        }))
    }

    async fn describe(
        &self,
        _request: Request<DescribeRequest>,
    ) -> Result<Response<DescribeResponse>, Status> {
        Ok(Response::new(DescribeResponse {
            supports_reference_update: self.model.supports_reference_update(),
            supports_scoring: self.model.supports_scoring(),
            model_dim: self.model.model_dim(),
            description: self.model.description(),
        }))
    }
}

/// Runs a sidecar on `addr` until the process is signalled.
///
/// Plaintext, matching `conflux-node`'s own local hop (ADR 0004): a
/// sidecar is normally colocated with the server it serves. A deployment
/// that separates them should put the sidecar behind TLS —
/// [`conflux_net::TrustedReferenceTransport::connect_with_tls`] is the
/// other half — because the trusted root dataset is the one input whose
/// integrity the entire FLTrust defense depends on.
pub async fn serve<M: TrustedModel + 'static>(
    addr: std::net::SocketAddr,
    model: M,
) -> Result<(), Box<dyn std::error::Error>> {
    let description = model.description();
    let service = TrustedReferenceService::new(model);

    tracing::info!(%addr, model = %description, "trusted-reference sidecar listening");

    tonic::transport::Server::builder()
        .add_service(TrustedReferenceServer::new(service))
        .serve_with_shutdown(addr, async {
            // Same shutdown posture as both binaries gained in Tier 5's
            // H3: a sidecar killed mid-request should close its listener
            // rather than vanish, so the server sees a closed connection
            // instead of a reset.
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown requested; closing the sidecar listener");
        })
        .await?;

    Ok(())
}
