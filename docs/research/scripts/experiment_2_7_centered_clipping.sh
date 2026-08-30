#!/usr/bin/env bash
# Section 2, Experiment 2.7 — places Centered Clipping (Karimireddy, He &
# Jaggi 2021; `CenteredClippingAggregator`, crates/conflux-core/src/
# temporal.rs, Phase 15) into the same comparison Experiments 2.2/2.4
# already built for the other cross-round methods.
#
# Centered Clipping was not part of docs/research/
# temporal-consistency-aggregation.md's original validation plan — it is a
# published method this framework ships, not a hypothesis this document
# proposes. It earns a place in this comparison for one structural reason:
# it is the third method here that carries state across rounds (after
# FoolsGold and DSS), and the only one of the three that uses that state
# to *bound* every client's influence rather than to *score* clients
# against each other. That difference is what these runs measure.
#
# Two questions, deliberately separated:
#   1. How does `centered_clipping` compare against the stateless
#      baselines (fedavg, krum) and the other temporal methods
#      (foolsgold, dss_fedavg) on the same attacks Experiment 2.4 used?
#   2. How sensitive is it to tau? Tau is the method's one tunable, it is
#      problem-scale dependent, and the paper tunes it per experiment —
#      so a single tau would report a tuning artifact as a property of
#      the method. The sweep below is the honest version of that claim.
#
# Honest-client updates here are ~N(1.0, 0.3) per coordinate at dim=3, so
# honest-to-honest deviations run around 0.3-0.9. The tau grid brackets
# that: 0.25 clips even honest clients, 1.0 sits at the top of the honest
# band, and 4.0/16.0 progressively stop binding at all (at which point
# the method must degenerate to a plain mean, per its own unit test).
#
# The two parts write to two separate results files on purpose.
# summarize.py groups by (aggregator, attack) and knows nothing about
# clip_radius, so mixing part 2's tau sweep into part 1's file would
# silently average four different taus into one "centered_clipping" row
# — a summary that looks fine and means nothing. Part 2 gets its own
# tau-aware summary instead (summarize_tau_sweep.py).
#
# Usage:
#   ./experiment_2_7_centered_clipping.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites both output files fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_7_centered_clipping.jsonl}"
NUM_REPEATS="${2:-5}"
TAU_OUT="${OUT%.jsonl}_tau_sweep.jsonl"
: > "$OUT"
: > "$TAU_OUT"

# Same attack set and design as Experiment 2.4, so the two results files
# are directly comparable row-for-row.
ATTACKS=(persistent_sybil adaptive_evasion scaling)
NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

# Part 1: centered_clipping at its builtin-fallback tau against every
# reference method. The comparators run at tau=1.0 too, but ignore it
# entirely — only `centered_clipping` reads clip_radius.
COMPARATORS=(fedavg krum multi_krum foolsgold dss_fedavg centered_clipping)

# Part 2: tau sensitivity, centered_clipping only.
TAUS=(0.25 1.0 4.0 16.0)

echo "=== Experiment 2.7 part 1: ${#COMPARATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
for attack in "${ATTACKS[@]}"; do
  for aggregator in "${COMPARATORS[@]}"; do
    echo "  running: $aggregator vs $attack" >&2
    for repeat in $(seq 1 "$NUM_REPEATS"); do
      base_seed=$(( (repeat - 1) * 1000 + 1 ))
      "$RUNNER" \
        --aggregator "$aggregator" \
        --attack "$attack" \
        --num-honest "$NUM_HONEST" \
        --num-attackers "$NUM_ATTACKERS" \
        --byzantine-fraction "$BYZANTINE_FRACTION" \
        --clip-radius 1.0 \
        --seed "$base_seed" \
        --rounds "$ROUNDS" \
        >> "$OUT"
    done
  done
done

echo "=== Experiment 2.7 part 2: tau sweep over ${TAUS[*]} ===" >&2
for attack in "${ATTACKS[@]}"; do
  for tau in "${TAUS[@]}"; do
    echo "  running: centered_clipping tau=$tau vs $attack" >&2
    for repeat in $(seq 1 "$NUM_REPEATS"); do
      base_seed=$(( (repeat - 1) * 1000 + 1 ))
      "$RUNNER" \
        --aggregator centered_clipping \
        --attack "$attack" \
        --num-honest "$NUM_HONEST" \
        --num-attackers "$NUM_ATTACKERS" \
        --byzantine-fraction "$BYZANTINE_FRACTION" \
        --clip-radius "$tau" \
        --seed "$base_seed" \
        --rounds "$ROUNDS" \
        >> "$TAU_OUT"
    done
  done
done

echo "=== done: $(wc -l < "$OUT") rows -> $OUT ===" >&2
echo "===       $(wc -l < "$TAU_OUT") rows -> $TAU_OUT ===" >&2
