//! The whole ADR 0011 boundary, end to end, over a real gRPC hop.
//!
//! A real sidecar process-equivalent (the real service, on a real port),
//! the real `conflux-net` client `conflux-server` would use, and a real
//! `FlTrustAggregator` consuming what comes back. Nothing is mocked; the
//! only thing unrealistic is that both ends are in one process.
//!
//! This test lives here rather than in `conflux-server` on purpose. ADR
//! 0011, following ADR 0010's precedent, requires that `conflux-server`
//! never depend on this crate at any depth — including through a
//! dev-dependency, since that is still an edge in the dependency graph.
//! Testing from this side proves the hop works and leaves that graph
//! untouched.

use std::sync::Arc;

use conflux_core::{Aggregator, FlTrustAggregator, TrustedReference};
use conflux_net::TrustedReferenceTransport;
use conflux_proto::trusted_reference_server::TrustedReferenceServer;
use conflux_proto::{ClientDelta, decode_weights, encode_weights};
use conflux_trusted_reference::{LinearLeastSquares, TrustedModel, TrustedReferenceService};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// `y = 2x₀ + 3x₁`. Small, exactly recoverable, and — the point — data no
/// client contributed to.
fn root_dataset() -> Vec<(Vec<f32>, f32)> {
    vec![
        (vec![1.0, 0.0], 2.0),
        (vec![0.0, 1.0], 3.0),
        (vec![1.0, 1.0], 5.0),
        (vec![2.0, 1.0], 7.0),
        (vec![1.0, 2.0], 8.0),
    ]
}

/// Starts a real sidecar and returns its address.
async fn spawn_sidecar<M: TrustedModel + 'static>(model: M) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(TrustedReferenceServer::new(TrustedReferenceService::new(
                model,
            )))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

fn delta(client_id: &str, weights: &[f32]) -> ClientDelta {
    ClientDelta {
        client_id: client_id.to_string(),
        round: 1,
        weights: encode_weights(weights),
        num_samples: 10,
        ..Default::default()
    }
}

#[tokio::test]
async fn the_server_can_fetch_a_reference_over_the_real_hop() {
    let addr = spawn_sidecar(LinearLeastSquares::new(root_dataset(), 0.05, 2000)).await;
    let mut client = TrustedReferenceTransport::connect(addr).await.unwrap();

    let global = vec![0.0_f32, 0.0];
    let reference = client
        .reference_update(1, encode_weights(&global))
        .await
        .expect("the sidecar answers");

    let weights = decode_weights(&reference).unwrap();
    assert_eq!(weights.len(), global.len());
    // The sidecar trained: it moved from [0, 0] toward the true [2, 3].
    assert!(
        (weights[0] - 2.0).abs() < 0.1 && (weights[1] - 3.0).abs() < 0.1,
        "got {weights:?}, expected ~[2, 3]"
    );
}

#[tokio::test]
async fn describe_reports_what_the_sidecar_can_do() {
    let addr = spawn_sidecar(LinearLeastSquares::new(root_dataset(), 0.05, 100)).await;
    let mut client = TrustedReferenceTransport::connect(addr).await.unwrap();

    let caps = client.describe().await.unwrap();
    assert!(caps.supports_reference_update);
    assert!(caps.supports_scoring);
    assert_eq!(caps.model_dim, Some(2));
    assert!(caps.description.contains("linear least squares"));
}

#[tokio::test]
async fn a_sidecar_that_cannot_serve_fltrust_says_so_rather_than_answering_badly() {
    // The startup-handshake case. A deployer who configures `fltrust`
    // against a scoring-only sidecar should find out at startup, not in
    // round one after clients have connected.
    struct ScoringOnly;
    impl TrustedModel for ScoringOnly {
        fn train_reference(&self, g: &[f32]) -> Vec<f32> {
            g.to_vec()
        }
        fn score(&self, _g: &[f32], _c: &[f32]) -> f32 {
            0.0
        }
        fn supports_reference_update(&self) -> bool {
            false
        }
    }

    let addr = spawn_sidecar(ScoringOnly).await;
    let mut client = TrustedReferenceTransport::connect(addr).await.unwrap();

    let caps = client.describe().await.unwrap();
    assert!(!caps.supports_reference_update);
    assert!(caps.supports_scoring);

    // And calling it anyway is refused explicitly rather than returning
    // the input dressed up as a reference.
    let err = client
        .reference_update(1, encode_weights(&[0.0, 0.0]))
        .await
        .expect_err("must refuse");
    assert!(
        format!("{err}").contains("fltrust"),
        "the error should name what cannot be served: {err}"
    );
}

#[tokio::test]
async fn zeno_style_scoring_ranks_candidates_over_the_hop() {
    let addr = spawn_sidecar(LinearLeastSquares::new(root_dataset(), 0.05, 100)).await;
    let mut client = TrustedReferenceTransport::connect(addr).await.unwrap();

    let global = encode_weights(&[0.0_f32, 0.0]);
    let scores = client
        .score_updates(
            4,
            global,
            vec![
                ("good".to_string(), encode_weights(&[2.0, 3.0])),
                ("mediocre".to_string(), encode_weights(&[1.0, 1.5])),
                ("harmful".to_string(), encode_weights(&[-6.0, 11.0])),
            ],
        )
        .await
        .unwrap();

    let lookup = |id: &str| scores.iter().find(|(c, _)| c == id).unwrap().1;
    assert!(lookup("good") > lookup("mediocre"));
    assert!(lookup("mediocre") > lookup("harmful"));
    assert!(
        lookup("harmful") < 0.0,
        "a candidate worse than the global model must score negative"
    );
}

#[tokio::test]
async fn a_stale_round_from_the_sidecar_is_caught_not_used() {
    // A reference from the wrong round is a well-formed vector of the
    // right length. Using it would weaken the defense silently rather
    // than fail, which is why the client checks the echoed round.
    struct WrongRound;
    impl TrustedModel for WrongRound {
        fn train_reference(&self, g: &[f32]) -> Vec<f32> {
            g.to_vec()
        }
        fn score(&self, _g: &[f32], _c: &[f32]) -> f32 {
            0.0
        }
    }

    // A service that echoes a fixed round, simulating a lagging sidecar.
    use conflux_proto::trusted_reference_server::TrustedReference as TrustedReferenceRpc;
    use conflux_proto::{
        DescribeRequest, DescribeResponse, ReferenceRequest, ReferenceUpdate, ScoreRequest,
        ScoreResponse,
    };
    use tonic::{Request, Response, Status};

    struct LaggingSidecar;
    #[tonic::async_trait]
    impl TrustedReferenceRpc for LaggingSidecar {
        async fn get_reference_update(
            &self,
            request: Request<ReferenceRequest>,
        ) -> Result<Response<ReferenceUpdate>, Status> {
            let req = request.into_inner();
            Ok(Response::new(ReferenceUpdate {
                round: req.round.saturating_sub(1), // one round behind
                weights: req.global_weights,
                local_steps: None,
            }))
        }
        async fn score_updates(
            &self,
            request: Request<ScoreRequest>,
        ) -> Result<Response<ScoreResponse>, Status> {
            Ok(Response::new(ScoreResponse {
                round: request.into_inner().round.saturating_sub(1),
                scores: Vec::new(),
            }))
        }
        async fn describe(
            &self,
            _request: Request<DescribeRequest>,
        ) -> Result<Response<DescribeResponse>, Status> {
            Ok(Response::new(DescribeResponse::default()))
        }
    }
    let _ = WrongRound;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(TrustedReferenceServer::new(LaggingSidecar))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    let mut client = TrustedReferenceTransport::connect(format!("http://{addr}"))
        .await
        .unwrap();

    let err = client
        .reference_update(5, encode_weights(&[0.0, 0.0]))
        .await
        .expect_err("a reference for round 4 must not be accepted for round 5");
    let message = format!("{err}");
    assert!(
        message.contains("round 4") && message.contains("round 5"),
        "the error should name both rounds: {message}"
    );
}

#[tokio::test]
async fn the_full_path_a_real_round_would_take() {
    // Sidecar -> conflux-net client -> FlTrustAggregator -> aggregate.
    // Every hop the server's round pipeline would make, in order.
    let addr = spawn_sidecar(LinearLeastSquares::new(root_dataset(), 0.05, 2000)).await;
    let mut client = TrustedReferenceTransport::connect(addr).await.unwrap();

    let global = vec![0.0_f32, 0.0];

    // 1. The server fetches this round's reference.
    let reference_bytes = client
        .reference_update(1, encode_weights(&global))
        .await
        .unwrap();
    let reference_weights = decode_weights(&reference_bytes).unwrap();

    // 2. It injects it (ADR 0012's interior-mutability pattern — the
    //    aggregator is behind an Arc and `aggregate` takes `&self`).
    let aggregator = Arc::new(FlTrustAggregator::new());
    aggregator.set_reference(TrustedReference {
        global_weights: global.clone(),
        reference_weights: reference_weights.clone(),
    });

    // 3. A round arrives in which the attackers are the majority — three
    //    of four, colluding on a direction the trusted data contradicts.
    let batch = [
        delta("honest", &[1.9, 3.1]),
        delta("sybil-1", &[-8.0, -12.0]),
        delta("sybil-2", &[-8.1, -11.9]),
        delta("sybil-3", &[-7.9, -12.1]),
    ];

    let out = aggregator.aggregate(&batch).unwrap();

    // The majority loses. No batch-derived method can say that: three of
    // four clients agreeing *is* the batch's consensus. FLTrust never
    // asked the batch — the reference came from data none of them
    // touched.
    assert!(
        out[0] > 0.0 && out[1] > 0.0,
        "got {out:?}; the aggregate must follow the trusted reference, not the majority"
    );
    assert!(out.iter().all(|w| w.is_finite()), "got {out:?}");

    // And for contrast: plain averaging over the same batch goes the
    // attackers' way, which is the whole reason this boundary exists.
    let mean_0 = (1.9 + -8.0 + -8.1 + -7.9) / 4.0;
    assert!(
        mean_0 < 0.0,
        "the arithmetic mean of this batch is {mean_0}, i.e. the attackers win it"
    );
}
