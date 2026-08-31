//! Real integration tests for the round pipeline: a live gRPC server
//! driven by a real `conflux-net::PullTransport` client (standing in for
//! what `conflux-node` will be in Phase 6), the HTTP admin surface
//! exercised as real request/response round trips, and direct tests of
//! `run_round`'s edge cases.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use conflux_buffer::RoundBuffer;
use conflux_config::{AccountingScope, BudgetExhaustedAction, Mode, Overrides, Topology};
use conflux_net::{FlTransportService, PullTransport, RoundDispatcher};
use conflux_privacy::PrivacyAccountant;
use conflux_proto::fl_transport_server::FlTransportServer;
use conflux_proto::{DeltaChunk, decode_weights, encode_weights};
use conflux_registry::{ClientId, Registry};
use conflux_server::{AppState, ServerError, run_round};
use conflux_store::Store;
use http_body_util::BodyExt;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tower::ServiceExt;

fn deterministic_config(overrides: &Overrides) -> conflux_config::ResolvedConfig {
    let mut merged = overrides.clone();
    // No clipping/noise so the aggregated checkpoint is an exact,
    // assertable value rather than randomized by real DP noise.
    merged.clip_norm.get_or_insert(1000.0);
    merged.noise_multiplier.get_or_insert(0.0);
    merged.round_timeout_secs.get_or_insert(5);
    conflux_config::resolve(
        Topology::CrossDevice,
        Mode::Research,
        Some(("test", &merged)),
        &Overrides::default(),
        &Overrides::default(),
    )
    .unwrap()
}

async fn spawn_grpc(state: Arc<AppState>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(FlTransportServer::new(FlTransportService::new(state)))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

async fn wait_until_buffer_open(state: &AppState) {
    for _ in 0..200 {
        if state
            .current_buffer
            .lock()
            .expect("mutex poisoned")
            .is_some()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("run_round never opened this round's buffer");
}

#[tokio::test]
async fn end_to_end_single_round_pull_mode() {
    let config = deterministic_config(&Overrides::default());
    let initial_weights = vec![1.0, 2.0];
    let state = Arc::new(AppState::new(config, initial_weights.clone()));

    // Registered up front so `run_round`'s `active_clients()` sees exactly
    // one client and selects it.
    state
        .registry
        .register(ClientId("client-1".to_string()))
        .await
        .unwrap();

    let addr = spawn_grpc(Arc::clone(&state)).await;

    let round_state = Arc::clone(&state);
    let round_handle = tokio::spawn(async move { run_round(&round_state).await });
    wait_until_buffer_open(&state).await;

    let mut transport = PullTransport::connect(addr).await.unwrap();
    transport.register("client-1", "token").await.unwrap();

    let task = transport.fetch_task("client-1").await.unwrap();
    assert_eq!(task.round, 1);
    assert_eq!(
        decode_weights(&task.model_weights).unwrap(),
        initial_weights
    );

    let ack = transport
        .submit_delta(vec![DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[10.0, 20.0]),
            num_samples: 5,
        }])
        .await
        .unwrap();
    assert!(ack.accepted);

    let summary = round_handle.await.unwrap().unwrap();
    assert_eq!(summary.round, 1);
    assert_eq!(summary.num_submitted, 1);
    assert_eq!(summary.num_passed, 1);

    // One update, no clipping/noise: FedAvg of a single update is that
    // update's weights unchanged (confirmed for conflux-core in Phase 4).
    let checkpoint = state.store.load_latest_weights().await.unwrap();
    assert_eq!(checkpoint, vec![10.0, 20.0]);
    assert_eq!(state.round.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_round_completes_when_the_client_already_applied_its_own_privacy_transform() {
    // Phase 17's composition check. `conflux-node` can now clip and noise
    // an update before it leaves the node, and the server-side transform
    // still runs on top of whatever arrives. The claim under test is
    // narrow and deliberately so: the two stages compose without the
    // round erroring or producing a degenerate checkpoint. Whether
    // transforming twice is a *good* privacy/utility tradeoff is a
    // research question, not a correctness bar.
    let config = deterministic_config(&Overrides {
        privacy_mechanism: Some("gaussian_clipping".to_string()),
        clip_norm: Some(1.0),
        noise_multiplier: Some(0.01),
        ..Default::default()
    });
    let state = Arc::new(AppState::new(config, vec![1.0, 2.0]));
    state
        .registry
        .register(ClientId("client-1".to_string()))
        .await
        .unwrap();

    let addr = spawn_grpc(Arc::clone(&state)).await;
    let round_state = Arc::clone(&state);
    let round_handle = tokio::spawn(async move { run_round(&round_state).await });
    wait_until_buffer_open(&state).await;

    let mut transport = PullTransport::connect(addr).await.unwrap();
    transport.register("client-1", "token").await.unwrap();
    transport.fetch_task("client-1").await.unwrap();

    // Stands in for what `NodeBridge::with_local_privacy` now does before
    // submitting — the same mechanism, applied client-side first.
    let mut weights = vec![10.0f32, 20.0];
    let client_side = conflux_privacy::GaussianClippingPrivacy {
        clip_norm: 1.0,
        noise_multiplier: 0.01,
    };
    let mut rng = <rand::rngs::StdRng as rand::SeedableRng>::seed_from_u64(11);
    client_side.transform(&mut weights, &mut rng);

    let ack = transport
        .submit_delta(vec![DeltaChunk {
            client_id: "client-1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&weights),
            num_samples: 5,
        }])
        .await
        .unwrap();
    assert!(ack.accepted);

    let summary = round_handle.await.unwrap().unwrap();
    assert_eq!(summary.round, 1);
    assert_eq!(summary.num_submitted, 1);
    assert_eq!(summary.num_passed, 1);

    let checkpoint = state.store.load_latest_weights().await.unwrap();
    assert_eq!(checkpoint.len(), 2);
    assert!(
        checkpoint.iter().all(|w| w.is_finite()),
        "twice-transformed weights must stay finite, got {checkpoint:?}"
    );
    assert_eq!(state.round.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn health_endpoint_returns_ok() {
    let config = deterministic_config(&Overrides::default());
    let state = Arc::new(AppState::new(config, vec![0.0]));
    let router = conflux_server::router(state, None);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn round_status_endpoint_reports_current_round() {
    let config = deterministic_config(&Overrides::default());
    let state = Arc::new(AppState::new(config, vec![0.0]));
    let router = conflux_server::router(state, None);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/round/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["round"], 1);
}

#[tokio::test]
async fn budget_exhausted_halts_without_touching_store_or_registry() {
    let overrides = Overrides {
        budget_exhausted_action: Some(BudgetExhaustedAction::Halt),
        target_epsilon: Some(0.5),
        ..Default::default()
    };
    let config = deterministic_config(&overrides);

    let initial_weights = vec![7.0, 8.0];
    let state = Arc::new(AppState::new(config, initial_weights.clone()));

    // Exhaust the budget before `run_round` ever checks it.
    {
        let mut accountant = state.accountant.lock().unwrap();
        for _ in 0..50 {
            accountant.record_round(1.0, 0.1);
        }
    }

    let err = run_round(&state).await.unwrap_err();
    assert!(matches!(err, ServerError::BudgetExhausted));

    // Round never advanced and the checkpoint is untouched — proof
    // `run_round` bailed out before doing any real work.
    assert_eq!(state.round.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.store.load_latest_weights().await.unwrap(),
        initial_weights
    );
}

// Phase 14: PerClient accounting.

#[tokio::test]
async fn per_client_budget_excludes_only_the_exhausted_client_when_continuing() {
    let overrides = Overrides {
        accounting_scope: Some(AccountingScope::PerClient),
        budget_exhausted_action: Some(BudgetExhaustedAction::ContinueWithoutGuarantee),
        target_epsilon: Some(0.5),
        ..Default::default()
    };
    let config = deterministic_config(&overrides);
    let state = Arc::new(AppState::new(config, vec![0.0, 0.0]));

    state
        .registry
        .register(ClientId("healthy-client".to_string()))
        .await
        .unwrap();
    state
        .registry
        .register(ClientId("exhausted-client".to_string()))
        .await
        .unwrap();

    // Pre-exhaust exactly one client's own budget — the other client
    // has no recorded rounds at all, so it isn't exhausted.
    {
        let mut accountant = state.accountant.lock().unwrap();
        for _ in 0..50 {
            accountant.record_round_for_client("exhausted-client", 1.0, 0.1);
        }
    }

    let addr = spawn_grpc(Arc::clone(&state)).await;
    let round_state = Arc::clone(&state);
    let round_handle = tokio::spawn(async move { run_round(&round_state).await });
    wait_until_buffer_open(&state).await;

    let mut healthy = PullTransport::connect(addr.clone()).await.unwrap();
    healthy.register("healthy-client", "token").await.unwrap();
    healthy.fetch_task("healthy-client").await.unwrap();
    healthy
        .submit_delta(vec![DeltaChunk {
            client_id: "healthy-client".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[10.0, 20.0]),
            num_samples: 5,
        }])
        .await
        .unwrap();

    let mut exhausted = PullTransport::connect(addr).await.unwrap();
    exhausted
        .register("exhausted-client", "token")
        .await
        .unwrap();
    exhausted.fetch_task("exhausted-client").await.unwrap();
    exhausted
        .submit_delta(vec![DeltaChunk {
            client_id: "exhausted-client".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[1000.0, 2000.0]), // would be obvious if it leaked through
            num_samples: 5,
        }])
        .await
        .unwrap();

    let summary = round_handle.await.unwrap().unwrap();
    assert_eq!(summary.num_submitted, 2, "both clients submitted");
    assert_eq!(
        summary.num_passed, 1,
        "only the non-exhausted client's update was admitted"
    );

    // FedAvg of exactly one update (healthy-client's) is that update
    // unchanged — confirms exhausted-client's update never reached
    // aggregation, not just that the count matched.
    let checkpoint = state.store.load_latest_weights().await.unwrap();
    assert_eq!(checkpoint, vec![10.0, 20.0]);

    // The round itself still completed normally — ContinueWithoutGuarantee
    // excludes the one client, it doesn't fail the round.
    assert_eq!(state.round.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn per_client_budget_halts_the_round_when_any_client_is_exhausted() {
    let overrides = Overrides {
        accounting_scope: Some(AccountingScope::PerClient),
        budget_exhausted_action: Some(BudgetExhaustedAction::Halt),
        target_epsilon: Some(0.5),
        ..Default::default()
    };
    let config = deterministic_config(&overrides);
    let initial_weights = vec![7.0, 8.0];
    let state = Arc::new(AppState::new(config, initial_weights.clone()));

    state
        .registry
        .register(ClientId("exhausted-client".to_string()))
        .await
        .unwrap();

    {
        let mut accountant = state.accountant.lock().unwrap();
        for _ in 0..50 {
            accountant.record_round_for_client("exhausted-client", 1.0, 0.1);
        }
    }

    let addr = spawn_grpc(Arc::clone(&state)).await;
    let round_state = Arc::clone(&state);
    let round_handle = tokio::spawn(async move { run_round(&round_state).await });
    wait_until_buffer_open(&state).await;

    let mut transport = PullTransport::connect(addr).await.unwrap();
    transport
        .register("exhausted-client", "token")
        .await
        .unwrap();
    transport.fetch_task("exhausted-client").await.unwrap();
    transport
        .submit_delta(vec![DeltaChunk {
            client_id: "exhausted-client".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 1,
            data: encode_weights(&[10.0, 20.0]),
            num_samples: 5,
        }])
        .await
        .unwrap();

    let err = round_handle.await.unwrap().unwrap_err();
    assert!(matches!(
        err,
        ServerError::BudgetExhaustedForClient { client_id } if client_id == "exhausted-client"
    ));

    // Halt aborts before checkpointing — same "nothing touched" guarantee
    // Global's own Halt case already gives.
    assert_eq!(state.round.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.store.load_latest_weights().await.unwrap(),
        initial_weights
    );
}

#[tokio::test]
async fn submit_delta_reassembles_out_of_order_chunks() {
    let config = deterministic_config(&Overrides::default());
    let state = Arc::new(AppState::new(config, vec![0.0, 0.0, 0.0, 0.0]));

    let buffer = Arc::new(RoundBuffer::new(1, 1));
    *state.current_buffer.lock().unwrap() = Some(Arc::clone(&buffer));

    let full = [1.0f32, 2.0, 3.0, 4.0];
    let bytes = encode_weights(&full);
    let midpoint = bytes.len() / 2;
    let chunks = vec![
        // chunk 1 arrives before chunk 0
        DeltaChunk {
            client_id: "c1".to_string(),
            round: 1,
            chunk_index: 1,
            total_chunks: 2,
            data: bytes[midpoint..].to_vec(),
            num_samples: 3,
        },
        DeltaChunk {
            client_id: "c1".to_string(),
            round: 1,
            chunk_index: 0,
            total_chunks: 2,
            data: bytes[..midpoint].to_vec(),
            num_samples: 3,
        },
    ];

    RoundDispatcher::submit_delta(state.as_ref(), chunks)
        .await
        .unwrap();

    let flush = buffer.await_flush(Duration::from_millis(50)).await;
    assert_eq!(flush.deltas.len(), 1);
    assert_eq!(
        decode_weights(&flush.deltas[0].weights).unwrap(),
        full.to_vec()
    );
}

/// `docker run -d --name conflux-dev-postgres -e POSTGRES_PASSWORD=conflux
/// -e POSTGRES_DB=conflux -p 15432:5432 postgres:16-alpine` — see
/// `docs/phases/phase-7d-accountant-persistence.md`.
const TEST_POSTGRES_URL: &str = "postgres://postgres:conflux@127.0.0.1:15432/conflux";

#[tokio::test]
async fn restarted_server_replays_privacy_rounds_instead_of_resetting_epsilon() {
    let table = format!(
        "conflux_checkpoints_test_accountant_restart_{}",
        std::process::id()
    );
    let delta = deterministic_config(&Overrides::default()).delta.value;

    // First "process": a real AppState with persistent accounting, its
    // accountant advanced through the real `record_round_privacy_cost`
    // path (via `run_round`'s pipeline, exercised earlier in this file),
    // simulated here by driving the accountant + log directly, which is
    // exactly what that function does internally.
    let state_a = AppState::new_with_persistent_accounting_table(
        deterministic_config(&Overrides::default()),
        vec![0.0],
        TEST_POSTGRES_URL,
        &table,
    )
    .await
    .expect("connect to the dev Postgres container — is it running?");

    let epsilon_before_any_rounds = conflux_privacy::PrivacyAccountant::current_epsilon(
        &*state_a.accountant.lock().unwrap(),
        delta,
    );
    assert_eq!(
        epsilon_before_any_rounds, 0.0,
        "fresh table, nothing to replay yet"
    );

    for _ in 0..5 {
        {
            let mut accountant = state_a.accountant.lock().unwrap();
            conflux_privacy::PrivacyAccountant::record_round(&mut *accountant, 1.0, 0.5);
        }
        conflux_store::PrivacyRoundLog::append_round(
            state_a.accountant_log.as_ref().unwrap().as_ref(),
            1.0,
            0.5,
        )
        .await
        .unwrap();
    }
    let epsilon_after_five_rounds = conflux_privacy::PrivacyAccountant::current_epsilon(
        &*state_a.accountant.lock().unwrap(),
        delta,
    );
    assert!(epsilon_after_five_rounds > epsilon_before_any_rounds);

    // "Restart": a second, entirely independent AppState constructed
    // against the *same* table, simulating a fresh process start. If this
    // were still Phase 5/7b's in-memory-only accounting, this would start
    // at zero rounds recorded regardless of what the first instance did —
    // that's the actual gap this phase closes.
    let state_b = AppState::new_with_persistent_accounting_table(
        deterministic_config(&Overrides::default()),
        vec![0.0],
        TEST_POSTGRES_URL,
        &table,
    )
    .await
    .unwrap();
    let epsilon_after_restart = conflux_privacy::PrivacyAccountant::current_epsilon(
        &*state_b.accountant.lock().unwrap(),
        delta,
    );

    assert_eq!(epsilon_after_restart, epsilon_after_five_rounds);
    assert!(epsilon_after_restart > epsilon_before_any_rounds);
}

const TEST_REDIS_URL: &str = "redis://127.0.0.1:16379";

#[tokio::test]
async fn app_state_connect_builds_a_working_state_against_real_redis_and_postgres() {
    // `AppState::connect`'s public API deliberately doesn't expose a
    // per-test key/table override (each backend's own default, matching
    // every backend's "argument-based, not conflux-config-driven"
    // precedent) — so this test isolates itself by using values unique to
    // this run instead of relying on a clean shared default key/table.
    let client_id = ClientId(format!("connect-test-client-{}", std::process::id()));
    let round = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs(); // always higher than any previous run's round — real time only moves forward

    let state = conflux_server::AppState::connect(
        deterministic_config(&Overrides::default()),
        Mode::Production,
        vec![1.0, 2.0],
        conflux_server::BackendSelection {
            registry: conflux_server::RegistryBackend::Redis {
                url: TEST_REDIS_URL.to_string(),
            },
            store: conflux_server::StoreBackend::Postgres {
                url: TEST_POSTGRES_URL.to_string(),
            },
            accounting: conflux_server::AccountingBackend::Postgres {
                url: TEST_POSTGRES_URL.to_string(),
            },
        },
    )
    .await
    .expect("connect to the dev Redis and Postgres containers — are they running?");

    state.registry.register(client_id.clone()).await.unwrap();
    assert!(
        state
            .registry
            .active_clients()
            .await
            .unwrap()
            .contains(&client_id)
    );

    state
        .store
        .save_checkpoint(round, &[3.0, 4.0])
        .await
        .unwrap();
    assert_eq!(
        state.store.load_latest_weights().await.unwrap(),
        vec![3.0, 4.0]
    );

    assert!(state.accountant_log.is_some());
}

#[tokio::test]
async fn app_state_connect_refuses_production_with_in_memory_registry() {
    // `.unwrap_err()` needs `AppState: Debug` for the panic message on the
    // Ok path, which isn't worth deriving across every field just for
    // this one assertion — match directly instead.
    let result = conflux_server::AppState::connect(
        deterministic_config(&Overrides::default()),
        Mode::Production,
        vec![0.0],
        conflux_server::BackendSelection::default(),
    )
    .await;

    match result {
        Err(conflux_server::AppStateError::BackendSelection(
            conflux_server::BackendSelectionError::ProductionRequiresDurableRegistry,
        )) => {}
        Err(other) => panic!("expected ProductionRequiresDurableRegistry, got {other}"),
        Ok(_) => panic!("expected an error, got a working AppState"),
    }
}

/// Phase 8c: the `/admin/allowlist` HTTP surface exercised as real
/// request/response round trips — add via `POST`, confirm via `GET`,
/// revoke via `DELETE`, confirm removal — same `tower::ServiceExt::
/// oneshot` pattern as `health_endpoint_returns_ok` above. A fresh
/// `Router` is built per request since `Router` isn't `Clone`-and-reuse
/// friendly across a `oneshot` call that consumes it.
#[tokio::test]
async fn admin_allowlist_add_list_revoke_round_trip() {
    let config = deterministic_config(&Overrides::default());
    let state = Arc::new(AppState::new(config, vec![0.0]));

    let add_body = serde_json::json!({
        "client_id": "client-1",
        "identity": { "kind": "shared_token", "token": "secret" }
    });
    let response = conflux_server::router(Arc::clone(&state), None)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/admin/allowlist")
                .header("content-type", "application/json")
                .body(Body::from(add_body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = conflux_server::router(Arc::clone(&state), None)
        .oneshot(
            Request::builder()
                .uri("/admin/allowlist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["client_ids"], serde_json::json!(["client-1"]));

    let response = conflux_server::router(Arc::clone(&state), None)
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/admin/allowlist/client-1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = conflux_server::router(Arc::clone(&state), None)
        .oneshot(
            Request::builder()
                .uri("/admin/allowlist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["client_ids"], serde_json::json!([]));
}
