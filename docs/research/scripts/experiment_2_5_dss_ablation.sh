#!/usr/bin/env bash
# Section 2, Experiment 2.5 (docs/research/temporal-consistency-aggregation.md
# §5.6): the mechanism ablation §7.3 called for once DSS existed —
# stability-only vs. collusion-only vs. the shipped AND-gate, isolating
# which of DSS's two signals actually does the discriminating work for
# each attack shape. Same design as Experiment 2.2/2.4 (8 honest, 2
# attackers, 20 rounds, 5 repeats) so results are directly comparable to
# those tables.
#
# Usage:
#   ./experiment_2_5_dss_ablation.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_5_dss_ablation.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

AGGREGATORS=(dss_fedavg dssstab_fedavg dsscoll_fedavg)
ATTACKS=(persistent_sybil adaptive_evasion)
NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

echo "=== Experiment 2.5: DSS ablation, ${ROUNDS} rounds x ${#AGGREGATORS[@]} variants x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
for attack in "${ATTACKS[@]}"; do
  for aggregator in "${AGGREGATORS[@]}"; do
    echo "  running: $aggregator vs $attack" >&2
    for repeat in $(seq 1 "$NUM_REPEATS"); do
      base_seed=$(( (repeat - 1) * 1000 + 1 ))
      "$RUNNER" \
        --aggregator "$aggregator" \
        --attack "$attack" \
        --num-honest "$NUM_HONEST" \
        --num-attackers "$NUM_ATTACKERS" \
        --byzantine-fraction "$BYZANTINE_FRACTION" \
        --seed "$base_seed" \
        --rounds "$ROUNDS" \
        >> "$OUT"
    done
  done
done

echo "=== done: $(wc -l < "$OUT") rows written to $OUT ===" >&2
