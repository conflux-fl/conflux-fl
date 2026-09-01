#!/usr/bin/env bash
# Section 2, Experiment 2.11 — the measurement task `r2` has been blocked
# on: does dropping DSS's stability conjunct reopen Claim 2?
#
# THE QUESTION. Three independent findings now point at the AND-gate as
# DSS's limiting component:
#
#   §5.6  the gate is numerically identical to stability-only
#   §5.12 collusion-only catches correlated Sybils that stability-only
#         misses entirely (1.09 vs 17.13)
#   §5.14 collusion-only is best or tied-best on all six attacks tested,
#         one to two orders of magnitude ahead of the shipped gate
#
# Every one of those is an *attack* measurement. None of them asks the
# question the stability conjunct exists to answer, which is Claim 2:
# **does a legitimately-different honest client get punished?** Flipping
# the gate on attack evidence alone would be optimizing the metric the
# gate was never there to protect.
#
# WHY THIS IS THE RIGHT TEST. With zero attackers, a minority of honest
# clients drawn from a shifted distribution are *correlated with each
# other* — they share a mean. DSS's collusion signal is cosine similarity
# between deviation traces, so a shifted honest minority is exactly the
# population that looks colluding. The stability conjunct is what
# currently stops them being penalized for it. Remove it, and the
# question is whether they lose influence.
#
# That is the failure mode Claim 2 names, reproduced deliberately.
#
# WHY EXPERIMENT 2.3 COULD NOT ANSWER IT. Two reasons, both fixed
# 2026-09-01:
#
#   1. `run_fairness_experiment.rs` called `build_aggregator` directly, so
#      it could not resolve a `dss_`/`dsscoll_` name at all.
#   2. It was single-round. `DssAggregator` returns stability 1.0 for any
#      client with fewer than two trace entries, so in one round its gate
#      cannot fire and it behaves exactly like its base. A `dss_` row
#      would have printed FedAvg's numbers and looked like a measurement.
#
# The runner now takes `--rounds`, and both arms of the leave-one-out
# comparison run the full round sequence against one aggregator instance,
# so cross-round state accumulates. `--rounds 1` still reproduces 2.3.
#
# READING THE RESULT. Compare minority influence against majority
# influence at each shift:
#   - both roughly equal      -> no fairness cost
#   - minority < majority     -> the minority is being down-weighted;
#                                Claim 2 is reopened for that variant
#   - minority > majority     -> the minority moves the aggregate more,
#                                which at zero attackers is not unfair,
#                                just leverage from being unusual
#
# `dssstab_` and `dss_` are expected to behave alike (§5.6). `fedavg` is
# the no-defense floor and `krum`/`foolsgold` the reference points 2.3
# already characterized.
#
# Usage:
#   ./experiment_2_11_temporal_fairness.sh [output.jsonl] [rounds] [seeds]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

echo "=== building run_fairness_experiment (release) ===" >&2
(cd "$REPO_ROOT" && cargo build --release -p conflux-attacks --example run_fairness_experiment 2>&1 | tail -5) >&2
FAIRNESS_RUNNER="$REPO_ROOT/target/release/examples/run_fairness_experiment"

OUT="${1:-$RESULTS_DIR/experiment_2_11_temporal_fairness.jsonl}"
ROUNDS="${2:-20}"
NUM_SEEDS="${3:-20}"
: > "$OUT"

# Same cohort shape and shift ladder as Experiment 2.3, so these rows are
# directly comparable to §5.4's numbers rather than needing their own
# baseline.
AGGREGATORS=(fedavg krum foolsgold dss_fedavg dssstab_fedavg dsscoll_fedavg flanders_krum)
SHIFTS=(0.0 0.5 1.0 1.5 2.0 3.0)
SEEDS=$(seq 1 "$NUM_SEEDS")
NUM_MAJORITY=6
NUM_MINORITY=2

total=$(( ${#AGGREGATORS[@]} * ${#SHIFTS[@]} * NUM_SEEDS ))
done_count=0

echo "=== Experiment 2.11: temporal non-IID fairness, ${ROUNDS} rounds ($total configurations) ===" >&2
for aggregator in "${AGGREGATORS[@]}"; do
  for shift in "${SHIFTS[@]}"; do
    for seed in $SEEDS; do
      "$FAIRNESS_RUNNER" \
        --aggregator "$aggregator" \
        --num-majority "$NUM_MAJORITY" \
        --num-minority "$NUM_MINORITY" \
        --shift "$shift" \
        --seed "$seed" \
        --rounds "$ROUNDS" \
        >> "$OUT"
      done_count=$((done_count + 1))
      if (( done_count % 100 == 0 )); then
        echo "  ...$done_count/$total" >&2
      fi
    done
  done
done

echo "=== done: $(wc -l < "$OUT") rows written to $OUT ===" >&2
