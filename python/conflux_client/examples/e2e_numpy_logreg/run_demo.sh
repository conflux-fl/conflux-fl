#!/usr/bin/env bash
# End-to-end demo (docs/E2E_TESTING.md, Option A): builds the Rust
# binaries, partitions a synthetic dataset across N clients, launches one
# conflux-server + N conflux-node + N trainer_client.py + 1 eval_client.py,
# and prints accuracy over rounds compared to a centralized baseline.
#
# Usage:
#   ./run_demo.sh [AGGREGATOR] [N_CLIENTS] [ROUNDS]
#   ./run_demo.sh krum 5 15 --poison               # last client is a Byzantine attacker
#   ./run_demo.sh krum 5 15 --poison --no-reputation
#       isolates the aggregator's OWN defense from conflux-reputation's
#       cosine-similarity filter, which has its own, separate,
#       real vulnerability to a first-round large-magnitude outlier —
#       see docs/E2E_TESTING.md's "A real finding" section. Without this
#       flag, the demo shows the pipeline exactly as shipped, including
#       that interaction.
#   ./run_demo.sh fedavg 5 15 --dirichlet                    # non-IID split, alpha=0.5
#   ./run_demo.sh fedavg 5 15 --dirichlet --dirichlet-alpha 0.1
#       lower alpha = more skewed per-client class distributions (more
#       realistically non-IID); default IID split otherwise.
#
# Run from this directory with the venv already active (see README.md).
set -euo pipefail

AGGREGATOR="${1:-fedavg}"
N_CLIENTS="${2:-5}"
ROUNDS="${3:-15}"
POISON=false
NO_REPUTATION=false
DIRICHLET=false
DIRICHLET_ALPHA=0.5
prev_arg=""
for arg in "$@"; do
  if [ "$arg" = "--poison" ]; then POISON=true; fi
  if [ "$arg" = "--no-reputation" ]; then NO_REPUTATION=true; fi
  if [ "$arg" = "--dirichlet" ]; then DIRICHLET=true; fi
  if [ "$prev_arg" = "--dirichlet-alpha" ]; then DIRICHLET_ALPHA="$arg"; fi
  prev_arg="$arg"
done
MIN_REPUTATION_SCORE=0.3
if [ "$NO_REPUTATION" = true ]; then MIN_REPUTATION_SCORE=-1.0; fi
SPLIT_FLAGS=(--split iid)
if [ "$DIRICHLET" = true ]; then SPLIT_FLAGS=(--split dirichlet --dirichlet-alpha "$DIRICHLET_ALPHA"); fi

FEATURES=10
DIM=$((FEATURES + 1)) # + bias
LR=0.5
STEPS=5

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
WORK_DIR="$(mktemp -d)"
PIDS=()

cleanup() {
  local exit_code=$?
  echo ""
  echo "=== cleaning up ==="
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  # Keep the work dir only when something went wrong — that is when you
  # would want to look at it. Keeping it unconditionally leaked ~22 MB
  # per run into /tmp, which on most Linux systems is tmpfs and therefore
  # *RAM*: a 20-run sweep quietly consumed half a gigabyte of memory that
  # nothing would ever reclaim. Override with CONFLUX_KEEP_WORK_DIR=1.
  if [ "${CONFLUX_KEEP_WORK_DIR:-0}" = "1" ] || [ "$exit_code" != "0" ]; then
    echo "work dir kept for inspection: $WORK_DIR"
  else
    rm -rf "$WORK_DIR"
  fi
}
trap cleanup EXIT

echo "=== 1. building conflux-server + conflux-node ==="
(cd "$REPO_ROOT" && cargo build -p conflux-server -p conflux-node 2>&1 | tail -5)
SERVER_BIN="$REPO_ROOT/target/debug/conflux-server"
NODE_BIN="$REPO_ROOT/target/debug/conflux-node"

echo ""
echo "=== 2. partitioning data (N=$N_CLIENTS clients, $FEATURES features, split=${SPLIT_FLAGS[1]}) ==="
python3 "$SCRIPT_DIR/partition_data.py" \
  --n-clients "$N_CLIENTS" --n-features "$FEATURES" --out-dir "$WORK_DIR" "${SPLIT_FLAGS[@]}"

echo ""
echo "=== 3. centralized baseline (target accuracy) ==="
(cd "$WORK_DIR" && python3 "$SCRIPT_DIR/centralized_baseline.py" \
  --total-steps "$((ROUNDS * STEPS))" --lr "$LR")

echo ""
echo "=== 4. starting conflux-server (aggregator=$AGGREGATOR, min_reputation_score=$MIN_REPUTATION_SCORE) ==="
CONFLUX_TOPOLOGY=cross_device \
CONFLUX_MODE=research \
CONFLUX_AGGREGATOR="$AGGREGATOR" \
CONFLUX_ROBUST_BYZANTINE_FRACTION=0.3 \
CONFLUX_MIN_REPUTATION_SCORE="$MIN_REPUTATION_SCORE" \
CONFLUX_QUORUM="$N_CLIENTS" \
CONFLUX_ROUND_TIMEOUT_SECS=30 \
CONFLUX_CLIP_NORM=1000 \
CONFLUX_NOISE_MULTIPLIER=0 \
CONFLUX_INITIAL_WEIGHTS_DIM="$DIM" \
RUST_LOG=warn \
"$SERVER_BIN" > "$WORK_DIR/server.log" 2>&1 &
PIDS+=($!)

for _ in $(seq 1 50); do
  if curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then break; fi
  sleep 0.2
done
if ! curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1; then
  echo "server did not become healthy — see $WORK_DIR/server.log"
  exit 1
fi
echo "server healthy"

echo ""
echo "=== 5. starting $N_CLIENTS conflux-node processes ==="
for i in $(seq 0 $((N_CLIENTS - 1))); do
  PORT=$((47100 + i))
  CONFLUX_CLIENT_ID="client-$i" \
  CONFLUX_LOCAL_ADDR="127.0.0.1:$PORT" \
  CONFLUX_SERVER_ADDR="http://127.0.0.1:50051" \
  RUST_LOG=warn \
  "$NODE_BIN" > "$WORK_DIR/node-$i.log" 2>&1 &
  PIDS+=($!)
done

echo "waiting for nodes to register..."
sleep 3

echo ""
echo "=== 6. starting $N_CLIENTS trainer clients + 1 eval client ==="
LAST=$((N_CLIENTS - 1))
for i in $(seq 0 "$LAST"); do
  PORT=$((47100 + i))
  POISON_FLAGS=()
  if [ "$POISON" = true ] && [ "$i" -eq "$LAST" ]; then
    echo "client-$i is a persistent Byzantine attacker (--poison)"
    POISON_FLAGS=(--poison --poison-magnitude 1000.0)
  fi
  python3 "$SCRIPT_DIR/trainer_client.py" \
    --address "127.0.0.1:$PORT" --client-id "client-$i" \
    --shard "$WORK_DIR/shard_$i.npz" --rounds "$ROUNDS" --lr "$LR" --steps "$STEPS" \
    "${POISON_FLAGS[@]}" \
    > "$WORK_DIR/trainer-$i.log" 2>&1 &
  PIDS+=($!)
done

EVAL_PORT=$((47100 + N_CLIENTS))
CONFLUX_CLIENT_ID="eval-node" \
CONFLUX_LOCAL_ADDR="127.0.0.1:$EVAL_PORT" \
CONFLUX_SERVER_ADDR="http://127.0.0.1:50051" \
RUST_LOG=warn \
"$NODE_BIN" > "$WORK_DIR/node-eval.log" 2>&1 &
PIDS+=($!)
sleep 1

python3 "$SCRIPT_DIR/eval_client.py" \
  --address "127.0.0.1:$EVAL_PORT" --held-out "$WORK_DIR/held_out.npz" \
  --rounds "$ROUNDS" --timeout 180 | tee "$WORK_DIR/eval.log"

echo ""
echo "=== done — final held-out accuracy above should be close to the centralized baseline printed in step 3 ==="
echo "logs: $WORK_DIR"
