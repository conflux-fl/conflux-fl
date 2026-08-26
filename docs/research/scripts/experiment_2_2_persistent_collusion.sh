#!/usr/bin/env bash
# Section 2, Experiment 2.2 (docs/research/temporal-consistency-aggregation.md):
# persistent/Sybil collusion across many rounds — the scenario
# PersistentSybilAttack exists for (see its own doc comment in
# crates/conflux-attacks/src/attacks.rs) — plus AdaptiveEvasionAttack,
# the harder case where the colluders also react to how well previous
# rounds went. Every aggregator, including `foolsgold` (the one method
# here with cross-round memory), against both attacks over many rounds,
# to see whether temporal defense actually holds up over time the way
# the research proposal hypothesizes — or doesn't; this script reports
# what happens, it doesn't assume it.
#
# Usage:
#   ./experiment_2_2_persistent_collusion.sh [output.jsonl] [num_repeats]
#
# §7.1 item 4 (statistical rigor): defaults to 5 independent 20-round
# trajectories per (aggregator, attack) now, not 1 — each repeat uses a
# non-overlapping base seed (repeat_index * 1000 + 1), so the 20 rounds
# within one repeat still each see a distinct honest batch (the runner
# itself offsets by `+round`), and repeats don't share any seed with
# each other either. `seed` in the output distinguishes repeats
# (rounds are already distinguished by `round`).
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_2_persistent_collusion.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

AGGREGATORS=(fedavg krum multi_krum trimmed_mean median faba bulyan geometric_median median_of_means divide_and_conquer foolsgold)
ATTACKS=(persistent_sybil adaptive_evasion)
NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

echo "=== Experiment 2.2: persistent collusion, ${ROUNDS} rounds x ${#AGGREGATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
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
