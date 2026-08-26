#!/usr/bin/env bash
# Section 2, Experiment 2.1 (docs/research/temporal-consistency-aggregation.md):
# collusion scaling — every shipped aggregator against every attack in
# conflux-attacks' repertoire, sweeping the number of colluding
# attackers, to build the real baseline ASR table Section 3 needs.
#
# Usage:
#   ./experiment_2_1_collusion_scaling.sh [output.jsonl] [num_seeds]
#
# §7.1 item 4 (statistical rigor): defaults to 5 seeds per configuration
# now, not 1 — each row carries its own `seed` field, so
# summarize.py's mean±CI columns are computed correctly across them.
#
# Re-runnable: each run truncates and rewrites the output file fresh
# (reproducible "latest results," not an accumulating log) — copy the
# file elsewhere first if you want to keep a previous run's numbers
# before re-running.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_1_collusion_scaling.jsonl}"
NUM_SEEDS="${2:-5}"
: > "$OUT"

AGGREGATORS=(fedavg krum multi_krum trimmed_mean median faba bulyan geometric_median median_of_means divide_and_conquer foolsgold)
ATTACKS=(gaussian sign_flipping scaling alie)
NUM_HONEST=8
NUM_ATTACKERS_SWEEP=(1 2 3 4)
BYZANTINE_FRACTION=0.2
ROUNDS=1

total=$(( ${#AGGREGATORS[@]} * ${#ATTACKS[@]} * ${#NUM_ATTACKERS_SWEEP[@]} * NUM_SEEDS ))
done_count=0

echo "=== Experiment 2.1: collusion scaling ($total configurations, $NUM_SEEDS seeds each) ===" >&2
for aggregator in "${AGGREGATORS[@]}"; do
  for attack in "${ATTACKS[@]}"; do
    for num_attackers in "${NUM_ATTACKERS_SWEEP[@]}"; do
      for seed in $(seq 1 "$NUM_SEEDS"); do
        "$RUNNER" \
          --aggregator "$aggregator" \
          --attack "$attack" \
          --num-honest "$NUM_HONEST" \
          --num-attackers "$num_attackers" \
          --byzantine-fraction "$BYZANTINE_FRACTION" \
          --seed "$seed" \
          --rounds "$ROUNDS" \
          >> "$OUT"
        done_count=$((done_count + 1))
        if (( done_count % 50 == 0 )); then
          echo "  ...$done_count/$total" >&2
        fi
      done
    done
  done
done

# Baseline: every aggregator with zero attackers, for comparison.
for aggregator in "${AGGREGATORS[@]}"; do
  for seed in $(seq 1 "$NUM_SEEDS"); do
    "$RUNNER" \
      --aggregator "$aggregator" \
      --attack none \
      --num-honest "$NUM_HONEST" \
      --num-attackers 0 \
      --byzantine-fraction "$BYZANTINE_FRACTION" \
      --seed "$seed" \
      --rounds "$ROUNDS" \
      >> "$OUT"
  done
done

echo "=== done: $(wc -l < "$OUT") rows written to $OUT ===" >&2
