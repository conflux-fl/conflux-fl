//! The server's client for the trusted-reference sidecar hop.
//!
//! This lives in `conflux-net` rather than in a crate of its own for a
//! deliberate reason: `conflux-server` must gain **zero** new
//! dependencies from FLTrust/Zeno existing. It already depends on
//! `conflux-net` for every other hop, so putting the client here means
//! the server can *call* a sidecar without ever depending on the crate
//! that *is* one — the same separation kept between `conflux-server`
//! and `conflux-attacks`.
//!
//! The connection is opened only when a deployer has configured an
//! aggregator that needs it. A deployment that has not is unchanged: no
//! sidecar process, no connection, no code path entered.

use conflux_proto::trusted_reference_client::TrustedReferenceClient;
use conflux_proto::{
    DescribeRequest, DescribeResponse, ReferenceRequest, ScoreCandidate, ScoreRequest,
};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::TransportError;

/// What a sidecar says it can do, learned once at startup.
///
/// Kept as a distinct type rather than passing the raw
/// [`DescribeResponse`] around so the server's startup validation reads
/// as a decision about capabilities, not as protobuf field access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidecarCapabilities {
    /// Whether the sidecar implements `GetReferenceUpdate` — required by
    /// FLTrust.
    pub supports_reference_update: bool,
    /// Whether it implements `ScoreUpdates` — required by Zeno.
    pub supports_scoring: bool,
    /// How many weights the sidecar's model has, when it knows in
    /// advance. `None` means it did not say, which is not an error — a
    /// sidecar that builds its model lazily legitimately cannot answer
    /// until it has seen a request.
    pub model_dim: Option<u64>,
    /// Free-text, for logs only. Never parsed.
    pub description: String,
}

impl From<DescribeResponse> for SidecarCapabilities {
    fn from(r: DescribeResponse) -> Self {
        Self {
            supports_reference_update: r.supports_reference_update,
            supports_scoring: r.supports_scoring,
            model_dim: r.model_dim,
            description: r.description,
        }
    }
}

/// A connection to a trusted-reference sidecar.
///
/// Deliberately thin: it moves flat `f32` buffers back and forth and has
/// no idea what model produced them, which is the whole point of putting
/// the capability in a sidecar. The sidecar knows what the model is; the
/// server still does not, so the server's model-opacity boundary survives
/// intact even though FLTrust and Zeno become possible.
pub struct TrustedReferenceTransport {
    client: TrustedReferenceClient<Channel>,
}

impl TrustedReferenceTransport {
    /// Connects without TLS. Appropriate when the sidecar runs beside the
    /// server — the same posture as `conflux-node`'s local loopback hop
    /// to its `ClientApp`, and for the same reason: a process on
    /// localhost talking to its own helper.
    pub async fn connect(addr: impl Into<String>) -> Result<Self, TransportError> {
        let client = TrustedReferenceClient::connect(addr.into()).await?;
        Ok(Self { client })
    }

    /// Connects with TLS, for a sidecar that is *not* on localhost.
    ///
    /// Worth having rather than assuming loopback: the sidecar holds the
    /// trusted root dataset, which is the one piece of data in the whole
    /// system whose integrity the entire defense rests on. A deployment
    /// that puts it on another host needs the hop authenticated, or
    /// FLTrust is anchored to whatever an on-path attacker returns.
    pub async fn connect_with_tls(
        addr: impl Into<String>,
        tls: ClientTlsConfig,
    ) -> Result<Self, TransportError> {
        let endpoint = Endpoint::from_shared(addr.into())?.tls_config(tls)?;
        let client = TrustedReferenceClient::new(endpoint.connect().await?);
        Ok(Self { client })
    }

    /// Asks the sidecar what it supports. Called once at startup.
    pub async fn describe(&mut self) -> Result<SidecarCapabilities, TransportError> {
        let response = self.client.describe(DescribeRequest {}).await?;
        Ok(response.into_inner().into())
    }

    /// FLTrust's reference update for `round`, trained from
    /// `global_weights`.
    ///
    /// Returns [`TransportError::StaleSidecarResponse`] if the sidecar
    /// answers for a different round than was asked. That check exists
    /// because the failure it catches is silent: a reference from an
    /// earlier round is a well-formed vector of the right length, and
    /// scoring this round's clients against it would quietly weaken the
    /// defense rather than break it.
    pub async fn reference_update(
        &mut self,
        round: u64,
        global_weights: Vec<u8>,
    ) -> Result<Vec<u8>, TransportError> {
        let response = self
            .client
            .get_reference_update(ReferenceRequest {
                round,
                global_weights,
            })
            .await?
            .into_inner();

        if response.round != round {
            return Err(TransportError::StaleSidecarResponse {
                expected: round,
                got: response.round,
            });
        }
        Ok(response.weights)
    }

    /// Zeno's per-client scores for this round's candidates.
    ///
    /// Returns `(client_id, score)` pairs. A sidecar that has no opinion
    /// about a candidate omits it, so the result may be shorter than
    /// `candidates` — the caller must treat a missing entry as "no
    /// score", which is not the same as a low one.
    pub async fn score_updates(
        &mut self,
        round: u64,
        global_weights: Vec<u8>,
        candidates: Vec<(String, Vec<u8>)>,
    ) -> Result<Vec<(String, f32)>, TransportError> {
        let response = self
            .client
            .score_updates(ScoreRequest {
                round,
                global_weights,
                candidates: candidates
                    .into_iter()
                    .map(|(client_id, weights)| ScoreCandidate { client_id, weights })
                    .collect(),
            })
            .await?
            .into_inner();

        if response.round != round {
            return Err(TransportError::StaleSidecarResponse {
                expected: round,
                got: response.round,
            });
        }
        Ok(response
            .scores
            .into_iter()
            .map(|s| (s.client_id, s.score))
            .collect())
    }
}
