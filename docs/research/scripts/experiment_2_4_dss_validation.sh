#!/usr/bin/env bash
# Section 2, Experiment 2.4 (docs/research/temporal-consistency-aggregation.md
# §6/§7.1 item 4): validates DSS (`DssAggregator`, crates/conflux-core/
# src/temporal.rs) against the now-complete real baseline from Experiment
# 2.2, by wrapping the same aggregators DSS is meant to complement and
# running them through the identical (attack x rounds x repeats) design.
# The question this answers: does wrapping a base method in DSS actually
# change its behavior under `persistent_sybil` (stable colluders — DSS's
# own doc comment and unit tests predict this should NOT be caught, since
# DSS only penalizes clients that are BOTH unstable AND colluding) and
# `adaptive_evasion` (escalate/retreat colluders — the erratic case DSS's
# hypothesis targets)? This script reports whatever the numbers show, it
# does not assume the hypothesis holds.
#
# `dss_<base>` is a run_experiment.rs-only naming convention (see its own
# `build_experiment_aggregator` doc comment) — DssAggregator is
# deliberately not in conflux-core's build_aggregator string catalog, so
# this prefix is stripped and the remainder is passed to build_aggregator
# to construct DSS's wrapped base.
#
# Usage:
#   ./experiment_2_4_dss_validation.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_4_dss_validation.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

# Paired (base, dss_base) so the summary can compare each base method
# directly against its own DSS-wrapped variant, plus foolsgold (the
# other cross-round-memory method) as a non-DSS temporal-defense
# reference point.
AGGREGATORS=(fedavg dss_fedavg krum dss_krum multi_krum dss_multi_krum foolsgold)
ATTACKS=(persistent_sybil adaptive_evasion)
NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

echo "=== Experiment 2.4: DSS validation, ${ROUNDS} rounds x ${#AGGREGATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
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
