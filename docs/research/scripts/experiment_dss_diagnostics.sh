#!/usr/bin/env bash
# §5.8 (solo-attacker mechanism analysis) and §5.9 (joint non-IID +
# attack) of docs/research/temporal-consistency-aggregation.md — both use
# `run_dss_diagnostics.rs` (crates/conflux-attacks/examples/), the
# per-client-diagnostics runner built specifically for these two
# sections, reading `DssAggregator::last_diagnostics()` after every
# round rather than reconstructing per-client weights via leave-one-out
# (which would corrupt a stateful aggregator's own history).
#
# Two output files:
#   - dss_diagnostics_solo_attacker.jsonl  — §5.8: base=fedavg vs.
#     base=krum, solo adaptive_evasion attacker, 5 seeds each.
#   - dss_diagnostics_joint.jsonl          — §5.9: non-IID minority +
#     2 colluding adaptive_evasion attackers, 5 seeds.
#
# Usage:
#   ./experiment_dss_diagnostics.sh [num_seeds]
#
# Re-runnable: truncates and rewrites both output files fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
RESULTS_DIR="$REPO_ROOT/docs/research/results"
mkdir -p "$RESULTS_DIR"

echo "=== building conflux-attacks' run_dss_diagnostics example (release) ===" >&2
(cd "$REPO_ROOT" && cargo build --release -p conflux-attacks --example run_dss_diagnostics 2>&1 | tail -5) >&2
RUNNER="$REPO_ROOT/target/release/examples/run_dss_diagnostics"

NUM_SEEDS="${1:-5}"

SOLO_OUT="$RESULTS_DIR/dss_diagnostics_solo_attacker.jsonl"
: > "$SOLO_OUT"
echo "=== §5.8: solo-attacker diagnostics, base in {fedavg, krum}, ${NUM_SEEDS} seeds ===" >&2
for base in fedavg krum; do
  for seed in $(seq 1 "$NUM_SEEDS"); do
    "$RUNNER" \
      --scenario attack \
      --base-aggregator "$base" \
      --attack adaptive_evasion \
      --num-majority 9 \
      --num-attackers 1 \
      --rounds 20 \
      --seed "$seed" \
      >> "$SOLO_OUT"
  done
done
echo "=== done: $(wc -l < "$SOLO_OUT") rows written to $SOLO_OUT ===" >&2

JOINT_OUT="$RESULTS_DIR/dss_diagnostics_joint.jsonl"
: > "$JOINT_OUT"
echo "=== §5.9: joint non-IID minority + attack diagnostics, base=fedavg, ${NUM_SEEDS} seeds ===" >&2
for seed in $(seq 1 "$NUM_SEEDS"); do
  "$RUNNER" \
    --scenario joint \
    --base-aggregator fedavg \
    --attack adaptive_evasion \
    --num-majority 6 \
    --num-minority 2 \
    --minority-shift 3.0 \
    --num-attackers 2 \
    --rounds 20 \
    --seed "$seed" \
    >> "$JOINT_OUT"
done
echo "=== done: $(wc -l < "$JOINT_OUT") rows written to $JOINT_OUT ===" >&2
