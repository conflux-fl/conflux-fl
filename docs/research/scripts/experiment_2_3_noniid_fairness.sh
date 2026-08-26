#!/usr/bin/env bash
# Experiment 2.3 (docs/research/temporal-consistency-aggregation.md,
# §3.3/§7.1): non-IID fairness — leave-one-out influence for majority vs.
# minority (shifted-mean) honest clients, zero attackers, swept across
# shift magnitude (a principled proxy for KL-divergence — see
# run_fairness_experiment.rs's own doc comment) and many seeds, since
# selection-based methods (Krum et al.) produce a sparse, mostly-zero
# leave-one-out signal per single run that only averages out
# meaningfully across repetitions.
#
# Usage:
#   ./experiment_2_3_noniid_fairness.sh [output.jsonl]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

# This experiment needs a second binary (different measurement shape —
# see run_fairness_experiment.rs) — build it alongside the one lib.sh
# already built.
echo "=== building run_fairness_experiment (release) ===" >&2
(cd "$REPO_ROOT" && cargo build --release -p conflux-attacks --example run_fairness_experiment 2>&1 | tail -5) >&2
FAIRNESS_RUNNER="$REPO_ROOT/target/release/examples/run_fairness_experiment"

OUT="${1:-$RESULTS_DIR/experiment_2_3_noniid_fairness.jsonl}"
: > "$OUT"

AGGREGATORS=(fedavg krum multi_krum trimmed_mean median faba bulyan geometric_median median_of_means divide_and_conquer foolsgold)
SHIFTS=(0.0 0.5 1.0 1.5 2.0 3.0)
SEEDS=$(seq 1 20)
NUM_MAJORITY=6
NUM_MINORITY=2

total=$(( ${#AGGREGATORS[@]} * ${#SHIFTS[@]} * 20 ))
done_count=0

echo "=== Experiment 2.3: non-IID fairness ($total configurations, 20 seeds each) ===" >&2
for aggregator in "${AGGREGATORS[@]}"; do
  for shift in "${SHIFTS[@]}"; do
    for seed in $SEEDS; do
      "$FAIRNESS_RUNNER" \
        --aggregator "$aggregator" \
        --num-majority "$NUM_MAJORITY" \
        --num-minority "$NUM_MINORITY" \
        --shift "$shift" \
        --seed "$seed" \
        >> "$OUT"
      done_count=$((done_count + 1))
      if (( done_count % 100 == 0 )); then
        echo "  ...$done_count/$total" >&2
      fi
    done
  done
done

echo "=== done: $(wc -l < "$OUT") rows written to $OUT ===" >&2
