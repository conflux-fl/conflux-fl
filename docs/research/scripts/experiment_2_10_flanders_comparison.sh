#!/usr/bin/env bash
# Section 2, Experiment 2.10 — DSS against FLANDERS, the closest
# published prior art.
#
# Why this exists: §6.5's novelty positioning was written against
# Centered Clipping, FoolsGold, FLTrust/Zeno and the single-round robust
# family. It did not cite FLANDERS (Gabrielli, Belli, Matrullo, Miori &
# Tolomei, 2024, arXiv 2303.16668), which is closer to DSS than any of
# them on three axes at once:
#
#   - It is a **cross-round temporal** defense, like DSS and unlike every
#     single-round method.
#   - It is a **pre-aggregation filter that wraps a base aggregator** —
#     which is DSS's own claimed contribution 3, "composability with any
#     existing Aggregator", almost verbatim.
#   - It explicitly targets **>50% malicious under non-IID**, which is
#     DSS's Claim 1 (the Sybil blind spot) and Claim 2 (non-IID
#     conflation) together.
#
# A novelty claim that does not engage with it is not a novelty claim.
# This experiment replaces the argument with numbers.
#
# What survives as a genuine distinction, and what this measures:
# FLANDERS's forecast is over the **full model matrix** (d parameters ×
# h clients), so its cost and its signal both scale with model size. DSS
# compares **scalar deviation traces** of length <= w (5 here), so it is
# model-size independent. That is a real difference in kind; the question
# is what it costs in detection.
#
# The design deliberately mirrors Experiments 2.4/2.8/2.9 — same client
# counts, same rounds, same seeds — so these rows sit directly beside the
# existing ones rather than needing their own baseline.
#
# Attacks, and why each is here:
#   persistent_sybil          identical colluders; DSS's best case (§5.5)
#   correlated_sybil          non-identical but temporally stable
#                             colluders; the case §5.12 built, where
#                             stability-only misses entirely
#   correlated_sybil_unstable the same, redrawn each round
#   scaling                   the 577x regression Finding 3 fixed (§5.11)
#   solo_attacker via gaussian  DSS's known open failure (§5.8.1)
#   alie                      a published attack neither method was
#                             designed against, as a neutral case
#
# Usage:
#   ./experiment_2_10_flanders_comparison.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_10_flanders_comparison.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

# Paired by base method, so every DSS row has a FLANDERS row computed on
# the identical batch. `fedavg` isolates each wrapper's own contribution;
# `krum` asks whether either still adds anything on top of a base that is
# already robust — the comparison Finding 3 showed DSS was previously
# failing (§5.11).
AGGREGATORS=(
  fedavg
  krum
  dss_fedavg
  flanders_fedavg
  dss_krum
  flanders_krum
  dsscoll_fedavg
  dssstab_fedavg
  foolsgold
)

# `adaptive_evasion` is DSS's own documented best case (§5.5: fedavg
# 553.0 -> 1.18) and is therefore the single most important row here: a
# comparison that omitted the attack the method was validated on would be
# choosing its ground. The two ablations (`dsscoll_`/`dssstab_`) are
# included because §5.12 showed the collusion half catches attacks the
# shipped AND-gate misses — so "what DSS's *mechanism* can do" and "what
# the shipped gate does" are different questions, and FLANDERS should be
# compared against both.
ATTACKS=(adaptive_evasion persistent_sybil correlated_sybil correlated_sybil_unstable scaling alie)

echo "=== Experiment 2.10: ${#AGGREGATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
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

# The second half: the regime FLANDERS was actually built for. Its
# headline claim is resilience when malicious clients *far exceed*
# legitimate ones — 60% and 80% in the paper's own experiments — which is
# precisely the regime DSS's Claim 1 says no single-round method can
# handle. Neither §5.5 nor §5.12 ever tested a majority-attacker batch,
# so this is new evidence for DSS as well, not only for the comparison.
echo "=== Experiment 2.10b: majority-attacker regime ===" >&2
for attack in adaptive_evasion persistent_sybil correlated_sybil; do
  for split in "6:4" "4:6" "2:8"; do
    honest="${split%%:*}"
    attackers="${split##*:}"
    fraction=$(python3 -c "print(f'{$attackers/($honest+$attackers):.2f}')")
    for aggregator in fedavg krum dss_fedavg dsscoll_fedavg flanders_fedavg foolsgold; do
      echo "  running: $aggregator vs $attack at ${attackers}/$((honest+attackers)) malicious" >&2
      for repeat in $(seq 1 "$NUM_REPEATS"); do
        base_seed=$(( (repeat - 1) * 1000 + 1 ))
        "$RUNNER" \
          --aggregator "$aggregator" \
          --attack "$attack" \
          --num-honest "$honest" \
          --num-attackers "$attackers" \
          --byzantine-fraction "$fraction" \
          --seed "$base_seed" \
          --rounds "$ROUNDS" \
          >> "$OUT"
      done
    done
  done
done

echo "=== done: $(wc -l < "$OUT") rows written to $OUT ===" >&2
