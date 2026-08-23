//! One round of spec §8's Step 0–5 pipeline, wiring every crate from
//! Phases 1–4 together. Does not loop — the caller (`main.rs`, or a test)
//! decides whether/when to run another round.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use conflux_buffer::{FlushReason, RoundBuffer};
use conflux_config::BudgetExhaustedAction;
use conflux_core::AggregatorError;
use conflux_privacy::PrivacyAccountant;
use conflux_proto::{ClientDelta, TaskResponse, encode_weights};
use conflux_registry::Registry;
use conflux_reputation::filter_by_threshold;
use conflux_store::Store;

use crate::{AppState, ServerError};

#[derive(Debug, Clone)]
pub struct RoundSummary {
    pub round: u64,
    pub flush_reason: FlushReason,
    pub num_selected: usize,
    pub num_submitted: usize,
    pub num_passed: usize,
}

pub async fn run_round(state: &Arc<AppState>) -> Result<RoundSummary, ServerError> {
    let round = state.round.load(Ordering::SeqCst);

    check_privacy_budget(state, round)?;

    let weights = state.store.load_latest_weights().await?;

    let active: Vec<String> = state
        .registry
        .active_clients()
        .await?
        .into_iter()
        .map(|id| id.0)
        .collect();

    // `quorum` has no universal default (spec §9); absent an override,
    // Phase 5 requires every selected client to respond — the
    // `cross_silo`/"all_available" ethos, not a formally-derived choice.
    let target_n = quorum_override(state).unwrap_or(active.len());
    let selected = state.selector.select(&active, target_n, round);
    let quorum = quorum_override(state).unwrap_or(selected.len());

    let buffer = Arc::new(RoundBuffer::new(round, quorum));
    *state
        .current_buffer
        .lock()
        .expect("app state mutex poisoned") = Some(Arc::clone(&buffer));

    // Push-mode subscribers get the task now; pull-mode clients see it on
    // their next `fetch_task`. No subscribers is not an error.
    let _ = state.push_sender.send(TaskResponse {
        task_id: format!("round-{round}"),
        round,
        model_weights: encode_weights(&weights),
    });

    let timeout = Duration::from_secs(state.config.round_timeout_secs.value);
    let flush = buffer.await_flush(timeout).await;

    let decoded = decode_flushed_deltas(&flush.deltas)?;
    let decoded = apply_server_side_privacy(state, decoded);

    let reference = mean_vector(&decoded);
    let min_reputation_score = state.config.min_reputation_score.value;
    let passed_ids: HashSet<String> = filter_by_threshold(
        &decoded,
        &reference,
        &state.reputation,
        min_reputation_score,
    )
    .into_iter()
    .collect();

    let filtered = reencode_passing_deltas(&flush.deltas, &decoded, &passed_ids);
    let num_submitted = flush.deltas.len();
    let num_passed = filtered.len();

    let new_weights = state.aggregator.aggregate(&filtered)?;
    state.store.save_checkpoint(round, &new_weights).await?;

    record_round_privacy_cost(state, selected.len(), active.len()).await?;

    state.round.store(round + 1, Ordering::SeqCst);
    *state
        .current_buffer
        .lock()
        .expect("app state mutex poisoned") = None;

    Ok(RoundSummary {
        round,
        flush_reason: flush.reason,
        num_selected: selected.len(),
        num_submitted,
        num_passed,
    })
}

fn quorum_override(state: &AppState) -> Option<usize> {
    state.config.quorum.as_ref().map(|q| q.value as usize)
}

fn check_privacy_budget(state: &AppState, round: u64) -> Result<(), ServerError> {
    let target_epsilon = state.config.target_epsilon.value;
    let delta = state.config.delta.value;
    let exhausted = state
        .accountant
        .lock()
        .expect("app state mutex poisoned")
        .budget_exhausted(target_epsilon, delta);
    if !exhausted {
        return Ok(());
    }
    match state.config.budget_exhausted_action.value {
        BudgetExhaustedAction::Halt => Err(ServerError::BudgetExhausted),
        BudgetExhaustedAction::ContinueWithoutGuarantee => {
            tracing::warn!(
                round,
                target_epsilon,
                delta,
                "privacy budget exhausted; continuing without guarantee"
            );
            Ok(())
        }
    }
}

fn decode_flushed_deltas(deltas: &[ClientDelta]) -> Result<Vec<(String, Vec<f32>)>, ServerError> {
    deltas
        .iter()
        .map(|delta| {
            conflux_proto::decode_weights(&delta.weights)
                .map(|w| (delta.client_id.clone(), w))
                .map_err(|_| {
                    ServerError::Aggregator(AggregatorError::MalformedWeights {
                        client_id: delta.client_id.clone(),
                        len: delta.weights.len(),
                    })
                })
        })
        .collect()
}

fn apply_server_side_privacy(
    state: &AppState,
    mut decoded: Vec<(String, Vec<f32>)>,
) -> Vec<(String, Vec<f32>)> {
    let mut rng = rand::rng();
    for (_, weights) in &mut decoded {
        state.privacy.transform(weights, &mut rng);
    }
    decoded
}

fn reencode_passing_deltas(
    deltas: &[ClientDelta],
    decoded: &[(String, Vec<f32>)],
    passed_ids: &HashSet<String>,
) -> Vec<ClientDelta> {
    deltas
        .iter()
        .zip(decoded)
        .filter(|(delta, _)| passed_ids.contains(&delta.client_id))
        .map(|(delta, (_, weights))| ClientDelta {
            client_id: delta.client_id.clone(),
            round: delta.round,
            weights: encode_weights(weights),
            num_samples: delta.num_samples,
        })
        .collect()
}

async fn record_round_privacy_cost(
    state: &AppState,
    num_selected: usize,
    num_active: usize,
) -> Result<(), ServerError> {
    let sample_rate = if num_active == 0 {
        0.0
    } else {
        num_selected as f32 / num_active as f32
    };
    let noise_multiplier = state.config.noise_multiplier.value;

    state
        .accountant
        .lock()
        .expect("app state mutex poisoned")
        .record_round(noise_multiplier, sample_rate);

    // Persisted immediately, not batched — a crash between updating the
    // in-memory accountant and this append would otherwise leave the
    // durable log one round behind, which is exactly the drift Phase 7d
    // exists to prevent. A persistence failure here is surfaced, not
    // swallowed: silently degrading back to in-memory-only accounting
    // would defeat the point of having `accountant_log` at all.
    if let Some(log) = &state.accountant_log {
        conflux_store::PrivacyRoundLog::append_round(log.as_ref(), noise_multiplier, sample_rate)
            .await?;
    }

    Ok(())
}

fn mean_vector(decoded: &[(String, Vec<f32>)]) -> Vec<f32> {
    let Some((_, first)) = decoded.first() else {
        return Vec::new();
    };
    let mut sum = vec![0.0f32; first.len()];
    for (_, weights) in decoded {
        for (s, w) in sum.iter_mut().zip(weights) {
            *s += w;
        }
    }
    let n = decoded.len() as f32;
    for s in &mut sum {
        *s /= n;
    }
    sum
}
