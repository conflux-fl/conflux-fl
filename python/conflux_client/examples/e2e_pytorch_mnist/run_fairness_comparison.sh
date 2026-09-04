#!/usr/bin/env bash
# Multi-seed SCAFFOLD / q-FedAvg / FedAvg comparison on non-IID MNIST,
# with per-client fairness metrics.
#
# Why this exists: the E2E harnesses guide reports a single-seed table
# (scaffold 0.902/0.940/0.020 vs fedavg 0.874/0.866/0.038) against a
# known ±0.06 run-to-run spread. This script is what turns that from a
# demo number into a defensible claim — or retracts it. Run it, read
# the summary, believe the summary.
#
# Usage:
#   ./run_fairness_comparison.sh                 # 3 arms × 5 seeds × 12 rounds (~35-45 min)
#   SEEDS="1 2 3" ROUNDS=8 ./run_fairness_comparison.sh
#   ARMS="fedavg scaffold" ./run_fairness_comparison.sh
#
# Env knobs: SEEDS, ARMS (fedavg|qfedavg|scaffold), ROUNDS, ALPHA
# (dirichlet), Q (qfedavg's q), OUT_DIR (results location).
#
# The seed varies BOTH the data partition and the trainers: each client
# gets --trainer-seed derived from (sweep seed, client index), so
# cross-seed variance includes real SGD stochasticity, not just the
# partition and round-timing races. (Model *init* stays shared across
# clients within a run — FL requires it — only batch sampling varies.)
set -uo pipefail

SEEDS="${SEEDS:-1 2 3 4 5}"
ARMS="${ARMS:-fedavg qfedavg scaffold}"
ROUNDS="${ROUNDS:-12}"
ALPHA="${ALPHA:-0.2}"
Q="${Q:-1.0}"
N=5
DIM=50890
GRPC_PORT="${CONFLUX_GRPC_PORT:-50051}"
ADMIN_PORT="${CONFLUX_ADMIN_PORT:-18080}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
OUT_DIR="${OUT_DIR:-$SCRIPT_DIR/fairness_results}"
mkdir -p "$OUT_DIR"
CSV="$OUT_DIR/fairness_comparison.csv"

echo "=== building conflux-server + conflux-node ==="
(cd "$REPO_ROOT" && cargo build -p conflux-server -p conflux-node 2>&1 | tail -1)
SERVER_BIN="$REPO_ROOT/target/debug/conflux-server"
NODE_BIN="$REPO_ROOT/target/debug/conflux-node"

PIDS=()
cleanup() {
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  PIDS=()
}
trap cleanup EXIT

# One (arm, seed) federation: real server, N nodes, N trainers, one
# eval client reporting global + per-client metrics each round.
run_one() {
  local arm="$1" seed="$2" work="$3"
  local trainer_flags=()
  local agg="$arm" q_env="0.0"
  case "$arm" in
    qfedavg)  q_env="$Q" ;;
    scaffold) trainer_flags=(--scaffold) ;;
  esac

  CONFLUX_TOPOLOGY=cross_device CONFLUX_MODE=research \
  CONFLUX_AGGREGATOR="$agg" CONFLUX_FAIRNESS_Q="$q_env" \
  CONFLUX_SERVER_LIPSCHITZ=1.0 CONFLUX_SCAFFOLD_NUM_CLIENTS=$N \
  CONFLUX_MIN_REPUTATION_SCORE=-1.0 CONFLUX_QUORUM=$N \
  CONFLUX_ROUND_TIMEOUT_SECS=60 CONFLUX_CLIP_NORM=1000 CONFLUX_NOISE_MULTIPLIER=0 \
  CONFLUX_INITIAL_WEIGHTS_DIM=$DIM \
  CONFLUX_HTTP_ADDR="127.0.0.1:$ADMIN_PORT" CONFLUX_GRPC_ADDR="127.0.0.1:$GRPC_PORT" \
  RUST_LOG=warn "$SERVER_BIN" > "$work/server.log" 2>&1 &
  PIDS+=($!)
  local server_pid=$!

  local ok=""
  for _ in $(seq 1 80); do
    if ! kill -0 "$server_pid" 2>/dev/null; then break; fi
    if curl -sf --max-time 2 "http://127.0.0.1:$ADMIN_PORT/health" 2>/dev/null \
        | grep -q '"status" *: *"ok"'; then ok=1; break; fi
    sleep 0.25
  done
  if [ -z "$ok" ]; then
    echo "  SERVER FAILED for $arm seed=$seed — tail of server.log:"
    tail -5 "$work/server.log" | sed 's/^/    /'
    return 1
  fi

  for i in $(seq 0 $((N - 1))); do
    CONFLUX_CLIENT_ID="client-$i" CONFLUX_LOCAL_ADDR="127.0.0.1:$((47100 + i))" \
    CONFLUX_SERVER_ADDR="http://127.0.0.1:$GRPC_PORT" RUST_LOG=warn \
      "$NODE_BIN" > "$work/node-$i.log" 2>&1 &
    PIDS+=($!)
  done
  CONFLUX_CLIENT_ID="eval-node" CONFLUX_LOCAL_ADDR="127.0.0.1:$((47100 + N))" \
  CONFLUX_SERVER_ADDR="http://127.0.0.1:$GRPC_PORT" RUST_LOG=warn \
    "$NODE_BIN" > "$work/node-eval.log" 2>&1 &
  PIDS+=($!)
  sleep 3

  for i in $(seq 0 $((N - 1))); do
    python3 "$SCRIPT_DIR/trainer_client.py" \
      --address "127.0.0.1:$((47100 + i))" --client-id "client-$i" \
      --shard "$work/shard_$i.pt" --rounds "$ROUNDS" --lr 0.1 --steps 30 \
      --trainer-seed "$((seed * 100 + i))" \
      "${trainer_flags[@]}" > "$work/trainer-$i.log" 2>&1 &
    PIDS+=($!)
  done

  local shards
  shards=$(ls "$work"/shard_*.pt | sort | paste -sd,)
  python3 "$SCRIPT_DIR/eval_client.py" \
    --address "127.0.0.1:$((47100 + N))" --held-out "$work/held_out.pt" \
    --rounds "$ROUNDS" --timeout 150 --shards "$shards" > "$work/eval.log" 2>&1

  cleanup
  sleep 2   # let the ports free before the next federation
  trap cleanup EXIT

  # The last reported round is the result row.
  local last
  last=$(grep "held_out_accuracy" "$work/eval.log" | tail -1)
  if [ -z "$last" ]; then
    echo "  NO EVAL OUTPUT for $arm seed=$seed — tail of eval.log:"
    tail -3 "$work/eval.log" | sed 's/^/    /'
    return 1
  fi
  local acc loss cmin cstd
  acc=$(echo "$last" | grep -oE "held_out_accuracy=[0-9.]+" | cut -d= -f2)
  loss=$(echo "$last" | grep -oE "held_out_loss=[0-9.]+" | cut -d= -f2)
  cmin=$(echo "$last" | grep -oE "client_acc_min=[0-9.]+" | cut -d= -f2)
  cstd=$(echo "$last" | grep -oE "client_acc_std=[0-9.]+" | cut -d= -f2)
  echo "$arm,$seed,$acc,$loss,$cmin,$cstd" >> "$CSV"
  echo "  $arm seed=$seed -> acc=$acc loss=$loss client_min=$cmin client_std=$cstd"
}

echo "arm,seed,held_out_acc,held_out_loss,client_acc_min,client_acc_std" > "$CSV"

for seed in $SEEDS; do
  WORK="$(mktemp -d)"
  echo ""
  echo "=== seed $seed: partitioning (dirichlet alpha=$ALPHA) ==="
  python3 "$SCRIPT_DIR/partition_data.py" --n-clients $N --out-dir "$WORK" \
    --split dirichlet --dirichlet-alpha "$ALPHA" --seed "$seed" 2>&1 | grep -c "shard" \
    | xargs -I{} echo "  {} shards written"
  for arm in $ARMS; do
    run_one "$arm" "$seed" "$WORK" || echo "  (arm $arm seed $seed FAILED — continuing)"
  done
  rm -rf "$WORK"   # tmpfs is RAM; never leave datasets behind
done

echo ""
echo "=== summary (mean ± sample std over seeds; higher acc/min better, lower std better) ==="
python3 - "$CSV" <<'PY'
import csv, statistics, sys
rows = list(csv.DictReader(open(sys.argv[1])))
arms = sorted({r["arm"] for r in rows}, key=lambda a: ["fedavg","qfedavg","scaffold"].index(a) if a in ["fedavg","qfedavg","scaffold"] else 99)
print(f"{'arm':<10}{'n':>3}  {'held_out_acc':>18}  {'client_min':>18}  {'client_std':>18}")
def fmt(vals):
    m = statistics.mean(vals)
    s = statistics.stdev(vals) if len(vals) > 1 else 0.0
    return f"{m:.4f} ± {s:.4f}"
for arm in arms:
    r = [x for x in rows if x["arm"] == arm]
    acc  = [float(x["held_out_acc"]) for x in r]
    cmin = [float(x["client_acc_min"]) for x in r]
    cstd = [float(x["client_acc_std"]) for x in r]
    print(f"{arm:<10}{len(r):>3}  {fmt(acc):>18}  {fmt(cmin):>18}  {fmt(cstd):>18}")
print(f"\nper-run rows: {sys.argv[1]}")
print("Read the ± before the means: with these seed counts, differences")
print("inside one std of each other are not differences.")
PY
