//! One round of spec §8's Step 0–5 pipeline, wiring every crate from
//! Phases 1–4 together. Does not loop — the caller (`main.rs`, or a test)
//! decides whether/when to run another round.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use conflux_buffer::{FlushReason, RoundBuffer};
use conflux_config::{AccountingScope, BudgetExhaustedAction};
use conflux_core::AggregatorError;
use conflux_privacy::PrivacyAccountant;
use conflux_proto::{ClientDelta, TaskResponse, encode_weights};
use conflux_registry::Registry;
use conflux_reputation::filter_by_threshold;
use conflux_store::Store;

use crate::{AppState, ServerError};

#[derive(Debug, Clone)]
/// What one round did, returned by `run_round` for logging and tests.
pub struct RoundSummary {
    /// Which round this describes.
    pub round: u64,
    /// Whether the round closed on quorum or on timeout.
    pub flush_reason: FlushReason,
    /// How many clients the selector picked.
    pub num_selected: usize,
    /// How many actually submitted before the buffer closed.
    pub num_submitted: usize,
    /// How many survived the reputation filter and privacy budget checks
    /// to reach aggregation.
    pub num_passed: usize,
}

/// Runs one full round: load the checkpoint, select clients, dispatch,
/// wait for quorum or timeout, filter, aggregate, checkpoint, and
/// advance the round counter.
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
    let decoded = filter_by_per_client_budget(state, decoded, round)?;
    let decoded = apply_server_side_privacy(state, decoded);

    // Phase 13: reputation filtering is opt-in, off by default — every
    // aggregator's default behavior should match its cited paper with
    // zero framework-imposed interference (see `docs/phases/
    // phase-13-reputation-reference-fix.md`). When off, every update
    // that survived `decode_flushed_deltas` above (decodable and finite)
    // goes straight to the configured aggregator, unmodified.
    let passed_ids: HashSet<String> = if state.config.reputation_filter_enabled.value {
        let reference = mean_vector(&decoded);
        let min_reputation_score = state.config.min_reputation_score.value;
        filter_by_threshold(
            &decoded,
            &reference,
            &state.reputation,
            min_reputation_score,
        )
        .into_iter()
        .collect()
    } else {
        decoded.iter().map(|(id, _)| id.clone()).collect()
    };

    let filtered = reencode_passing_deltas(&flush.deltas, &decoded, &passed_ids);
    let num_submitted = flush.deltas.len();
    let num_passed = filtered.len();

    let new_weights = state.aggregator.aggregate(&filtered)?;
    state.store.save_checkpoint(round, &new_weights).await?;

    let admitted_client_ids: Vec<&str> = filtered.iter().map(|d| d.client_id.as_str()).collect();
    record_round_privacy_cost(state, selected.len(), active.len(), &admitted_client_ids).await?;

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

/// The experiment-wide budget gate — only meaningful for
/// `AccountingScope::Global`. Phase 14: `PerClient` scope has no single
/// "the round" gate to check here; its enforcement moves to
/// [`filter_by_per_client_budget`], evaluated per client once the
/// round's batch is actually known, not once upfront against
/// candidates that haven't submitted yet.
fn check_privacy_budget(state: &AppState, round: u64) -> Result<(), ServerError> {
    if state.config.accounting_scope.value != AccountingScope::Global {
        return Ok(());
    }
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

/// Phase 14 (`AccountingScope::PerClient`): excludes any client whose
/// *own* cumulative epsilon has already reached `target_epsilon` from
/// this round's batch, before its update reaches server-side privacy or
/// aggregation — the same "exclude, don't fail the whole round" shape
/// [`decode_flushed_deltas`] already uses for non-finite values.
/// `budget_exhausted_action` still applies, but at client granularity:
/// `Halt` aborts the round the moment *any* client is over budget (a
/// strict posture — same as `Global`'s `Halt`, just triggered by one
/// client instead of the experiment total); `ContinueWithoutGuarantee`
/// drops just that client and keeps the round going with everyone else.
/// A no-op (returns `decoded` unchanged) under `AccountingScope::Global`.
fn filter_by_per_client_budget(
    state: &AppState,
    decoded: Vec<(String, Vec<f32>)>,
    round: u64,
) -> Result<Vec<(String, Vec<f32>)>, ServerError> {
    if state.config.accounting_scope.value != AccountingScope::PerClient {
        return Ok(decoded);
    }
    let target_epsilon = state.config.target_epsilon.value;
    let delta = state.config.delta.value;
    let accountant = state.accountant.lock().expect("app state mutex poisoned");

    let mut kept = Vec::with_capacity(decoded.len());
    for (client_id, weights) in decoded {
        if !accountant.budget_exhausted_for_client(&client_id, target_epsilon, delta) {
            kept.push((client_id, weights));
            continue;
        }
        match state.config.budget_exhausted_action.value {
            BudgetExhaustedAction::Halt => {
                return Err(ServerError::BudgetExhaustedForClient { client_id });
            }
            BudgetExhaustedAction::ContinueWithoutGuarantee => {
                tracing::warn!(
                    round,
                    client_id = %client_id,
                    target_epsilon,
                    delta,
                    "client excluded from round: its own privacy budget is exhausted"
                );
            }
        }
    }
    Ok(kept)
}

/// Decodes every delta (still a hard failure for the whole round if any
/// one is malformed at the byte level — a corrupt/truncated payload is a
/// protocol-level problem, not a numerically-degenerate-but-well-formed
/// value), then excludes — not fails the round over — any decoded update
/// containing a non-finite (`NaN`/`Inf`) value. Phase 13: a single
/// client with degenerate local data (e.g. a zero-sample shard from an
/// aggressive Dirichlet split) can otherwise poison
/// `conflux-reputation`'s shared batch-mean reference via `NaN`
/// propagation, rejecting every other client's honest update too — see
/// `docs/E2E_TESTING.md`'s "Real findings" #3. Excluded here,
/// unconditionally, before that reference is ever computed — regardless
/// of whether `reputation_filter_enabled` is on, since this is a plain
/// correctness bug, not a robustness policy choice.
fn decode_flushed_deltas(deltas: &[ClientDelta]) -> Result<Vec<(String, Vec<f32>)>, ServerError> {
    let decoded: Vec<(String, Vec<f32>)> = deltas
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
        .collect::<Result<_, _>>()?;

    Ok(decoded
        .into_iter()
        .filter(|(client_id, weights)| {
            let finite = weights.iter().all(|w| w.is_finite());
            if !finite {
                tracing::warn!(
                    client_id = %client_id,
                    "update excluded: contains a non-finite (NaN/Inf) value"
                );
            }
            finite
        })
        .collect())
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

/// Records this round's privacy cost — both the experiment-wide total
/// (`Global`'s own accounting, unchanged since Phase 7d) *and*, for
/// every client actually admitted into this round's aggregate, a
/// per-client entry (Phase 14). Both are recorded unconditionally,
/// regardless of `accounting_scope` — the same "never lose history the
/// current scope isn't actively using" reasoning `app_state.rs`'s
/// `connect_accounting` already applies on the replay side; a
/// deployment that switches `accounting_scope` between restarts should
/// find its other scope's history already there, not starting from
/// zero.
async fn record_round_privacy_cost(
    state: &AppState,
    num_selected: usize,
    num_active: usize,
    admitted_client_ids: &[&str],
) -> Result<(), ServerError> {
    let sample_rate = if num_active == 0 {
        0.0
    } else {
        num_selected as f32 / num_active as f32
    };
    let noise_multiplier = state.config.noise_multiplier.value;

    {
        let mut accountant = state.accountant.lock().expect("app state mutex poisoned");
        accountant.record_round(noise_multiplier, sample_rate);
        for &client_id in admitted_client_ids {
            accountant.record_round_for_client(client_id, noise_multiplier, sample_rate);
        }
    }

    // Persisted immediately, not batched — a crash between updating the
    // in-memory accountant and this append would otherwise leave the
    // durable log one round behind, which is exactly the drift Phase 7d
    // exists to prevent. A persistence failure here is surfaced, not
    // swallowed: silently degrading back to in-memory-only accounting
    // would defeat the point of having `accountant_log` at all.
    if let Some(log) = &state.accountant_log {
        conflux_store::PrivacyRoundLog::append_round(log.as_ref(), noise_multiplier, sample_rate)
            .await?;
        for &client_id in admitted_client_ids {
            conflux_store::PrivacyRoundLog::append_round_for_client(
                log.as_ref(),
                client_id,
                noise_multiplier,
                sample_rate,
            )
            .await?;
        }
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
