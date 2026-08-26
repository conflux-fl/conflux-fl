#!/usr/bin/env bash
# Section 2, Experiment 2.6 (docs/research/temporal-consistency-aggregation.md
# §5.7): a single, non-Sybil adaptive attacker — no colluding partner at
# all. Isolates whether DSS's protection in Experiment 2.4 required a
# *pair* of mutually-correlated colluders, or works for a lone erratic
# attacker too. Run against two bases: `fedavg` (fragile — the base's own
# reference point is not robust to a lone unfiltered attacker) and `krum`
# (already robust to n=1 Byzantine client on its own), to test whether
# DSS's own effectiveness here depends on the robustness of what it
# wraps.
#
# Usage:
#   ./experiment_2_6_solo_attacker.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_6_solo_attacker.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

AGGREGATORS=(fedavg dss_fedavg krum dss_krum foolsgold)
ATTACKS=(persistent_sybil adaptive_evasion)
NUM_HONEST=9
NUM_ATTACKERS=1
BYZANTINE_FRACTION=0.2
ROUNDS=20

echo "=== Experiment 2.6: solo (non-Sybil) attacker, ${ROUNDS} rounds x ${#AGGREGATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
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
