#!/usr/bin/env bash
# Section 2, Experiment 2.8 — does fixing Finding 3 actually fix it?
#
# Finding 3 (§5.5, confirmed independently in §5.7): DSS's combine step
# used the base method's output *only* as a deviation reference and never
# let it gate the final weights. So whenever DSS's own "unstable AND
# colluding" gate didn't fire — which is exactly what stable colluders
# are designed to avoid — every weight stayed 1.0 and the combine
# degraded to a plain weighted mean of every raw submission, discarding
# whatever the base method would have excluded. Measured consequence:
# `dss_krum` at 16.99 against `persistent_sybil`, ~57x worse than plain
# `krum`'s 0.297.
#
# The fix hands DSS's judgment back to the base method: re-weight the
# batch (drop fully-distrusted clients, scale the rest through
# `num_samples`) and call `base.aggregate` on it, so Krum still selects
# and Trimmed Mean still trims. A non-firing gate now degrades to *the
# base method* rather than to FedAvg.
#
# This script measures three variants side by side, so the claim is a
# comparison rather than a diff against a previous run:
#   <base>          — the base method alone, the floor DSS must not fall
#                     below
#   dssraw_<base>   — DSS with the original combine (the defect)
#   dss_<base>      — DSS with the fix
#
# The interesting comparison is dss_ vs the bare base: the fix's whole
# claim is that wrapping can no longer make things worse.
#
# Usage:
#   ./experiment_2_8_finding3_fix.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_8_finding3_fix.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

# Same design as Experiments 2.2/2.4, so rows are directly comparable to
# the numbers Finding 3 was originally measured from.
ATTACKS=(persistent_sybil adaptive_evasion scaling)
NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

# fedavg is included as the control: DSS-on-fedavg is the one
# configuration §5.5 found DSS genuinely helps, and the fix must not
# regress it. For fedavg specifically the two combines should agree
# closely — scaling num_samples by a weight *is* what a weighted mean
# does — so a large gap there would mean the fix broke something.
AGGREGATORS=(
  fedavg dssraw_fedavg dss_fedavg
  krum dssraw_krum dss_krum
  multi_krum dssraw_multi_krum dss_multi_krum
  trimmed_mean dssraw_trimmed_mean dss_trimmed_mean
)

echo "=== Experiment 2.8: ${#AGGREGATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
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
