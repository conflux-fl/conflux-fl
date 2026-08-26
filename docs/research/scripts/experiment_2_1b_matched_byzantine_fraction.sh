#!/usr/bin/env bash
# Section 5.2 follow-up (docs/research/temporal-consistency-aggregation.md):
# re-runs Experiment 2.1's collusion-scaling sweep with
# `byzantine_fraction` matched to the TRUE attacker fraction at each
# `num_attackers` step, instead of held fixed at 0.2 — isolating whether
# ScalingAttack's collapse of Multi-Krum/FABA/DnC/Trimmed-Mean (§5.2) was
# purely the parameter-mismatch artifact, or whether some residual
# attack advantage survives once every method is correctly parameterized
# for the actual threat it faces.
#
# Usage:
#   ./experiment_2_1b_matched_byzantine_fraction.sh [output.jsonl]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_1b_matched_byzantine_fraction.jsonl}"
: > "$OUT"

AGGREGATORS=(fedavg krum multi_krum trimmed_mean median faba bulyan geometric_median median_of_means divide_and_conquer foolsgold)
ATTACKS=(gaussian sign_flipping scaling alie)
NUM_HONEST=8
NUM_ATTACKERS_SWEEP=(1 2 3 4)
SEED=1
ROUNDS=1

total=$(( ${#AGGREGATORS[@]} * ${#ATTACKS[@]} * ${#NUM_ATTACKERS_SWEEP[@]} ))
done_count=0

echo "=== Experiment 2.1b: matched byzantine_fraction ($total configurations) ===" >&2
for aggregator in "${AGGREGATORS[@]}"; do
  for attack in "${ATTACKS[@]}"; do
    for num_attackers in "${NUM_ATTACKERS_SWEEP[@]}"; do
      total_clients=$((NUM_HONEST + num_attackers))
      # Match byzantine_fraction to the true attacker fraction, with a
      # small margin (+1 client's worth) so integer-floor rounding
      # inside each aggregator's own f = floor(byzantine_fraction * n)
      # doesn't itself re-underestimate by construction.
      byzantine_fraction=$(awk -v m="$num_attackers" -v n="$total_clients" 'BEGIN { f = (m + 1) / n; if (f > 0.49) f = 0.49; printf "%.4f", f }')
      "$RUNNER" \
        --aggregator "$aggregator" \
        --attack "$attack" \
        --num-honest "$NUM_HONEST" \
        --num-attackers "$num_attackers" \
        --byzantine-fraction "$byzantine_fraction" \
        --seed "$SEED" \
        --rounds "$ROUNDS" \
        >> "$OUT"
      done_count=$((done_count + 1))
      if (( done_count % 20 == 0 )); then
        echo "  ...$done_count/$total" >&2
      fi
    done
  done
done

echo "=== done: $(wc -l < "$OUT") rows written to $OUT ===" >&2
