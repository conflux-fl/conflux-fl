#!/usr/bin/env bash
# Section 2, Experiment 2.9 — does DSS's collusion signal add anything
# beyond its stability signal, once the attack stops making that question
# unanswerable?
#
# §5.6's mechanism ablation compared three DSS variants — the shipped
# AND-gate, stability-only (`dssstab_`), and collusion-only (`dsscoll_`)
# — and found the AND-gate numerically *identical* to stability-only. But
# it ran against `persistent_sybil`, where every colluder submits the
# byte-identical update. In that model every client's collusion score
# saturates, so the signal carries no information for anyone, and the
# result could not distinguish "collusion adds nothing" from "this attack
# makes collusion unmeasurable."
#
# `correlated_sybil` is the harder model that separates those: colluders
# pull toward a shared objective but each adds its own fixed offset, so
# they are correlated, individually distinguishable, and — because the
# offsets are fixed — temporally *stable*. A stability-only detector must
# miss them by construction. Anything that catches them is catching
# collusion specifically.
#
# The comparison that answers §5.6's open question is therefore
# `dsscoll_fedavg` vs `dssstab_fedavg` on the `correlated_sybil` row. If
# they are still identical, the collusion signal really is redundant. If
# collusion-only wins there, §5.6's conclusion was an artifact of its
# attack model, not a property of the mechanism.
#
# `persistent_sybil` is included as the control — it should reproduce
# §5.6's original numbers, confirming nothing else drifted. A
# `divergence` sweep varies how identical the colluders are, since
# `divergence = 0` is exactly `persistent_sybil` and the interesting
# question is where between the two the behavior changes.
#
# Usage:
#   ./experiment_2_9_correlated_sybils.sh [output.jsonl] [num_repeats]
#
# Re-runnable: truncates and rewrites the output file fresh each run.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib.sh
source "$SCRIPT_DIR/lib.sh"

OUT="${1:-$RESULTS_DIR/experiment_2_9_correlated_sybils.jsonl}"
NUM_REPEATS="${2:-5}"
: > "$OUT"

# Same design as Experiments 2.2/2.4/2.5, so rows stay comparable to
# §5.6's original ablation numbers.
NUM_HONEST=8
NUM_ATTACKERS=2
BYZANTINE_FRACTION=0.2
ROUNDS=20

# The three DSS variants §5.6 compared, plus the undefended baseline and
# two reference defenses.
AGGREGATORS=(fedavg dss_fedavg dssstab_fedavg dsscoll_fedavg krum foolsgold)

# persistent_sybil: the control (identical colluders, §5.6's model).
# correlated_sybil: non-identical, stable colluders — the new case.
# correlated_sybil_unstable: the contrast, where offsets are redrawn
#   every round so the group is unstable as well.
ATTACKS=(persistent_sybil correlated_sybil correlated_sybil_unstable)

echo "=== Experiment 2.9: ${#AGGREGATORS[@]} aggregators x ${#ATTACKS[@]} attacks x ${NUM_REPEATS} repeats ===" >&2
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
