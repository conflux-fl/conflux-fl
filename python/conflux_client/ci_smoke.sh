#!/usr/bin/env bash
# The gate that a broken Python client must not get past.
#
# Everything else CI runs on the Python side is static: syntax, lint,
# imports, unit tests. Those catch a lot, and they all pass for a client
# that connects, registers, and then silently sends nothing the server
# can use — which is exactly the failure this project has already
# shipped twice (stubs generated before ADR 0012's fields existed, and a
# server-side rebuild that dropped them again). Neither was visible
# without running the loop.
#
# So this runs the loop: a real conflux-server, a real conflux-node, and
# a real Python client over the real local gRPC hop, and then checks the
# server actually advanced a round on what the client sent.
#
# Usage:  ./ci_smoke.sh [ROUNDS]
# Env:    CONFLUX_ADMIN_PORT (default 18080), CONFLUX_GRPC_PORT (50251),
#         CONFLUX_LOCAL_PORT (47300)
set -uo pipefail

ROUNDS="${1:-2}"
DIM=8
ADMIN_PORT="${CONFLUX_ADMIN_PORT:-18080}"
GRPC_PORT="${CONFLUX_GRPC_PORT:-50251}"
LOCAL_PORT="${CONFLUX_LOCAL_PORT:-47300}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
WORK="$(mktemp -d)"
PIDS=()

cleanup() {
  local code=$?
  for pid in "${PIDS[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  wait 2>/dev/null || true
  if [ "$code" != "0" ]; then
    echo ""
    echo "--- server.log ---"; tail -30 "$WORK/server.log" 2>/dev/null
    echo "--- node.log ---";   tail -20 "$WORK/node.log" 2>/dev/null
    echo "--- client.log ---"; tail -20 "$WORK/client.log" 2>/dev/null
    echo ""
    echo "work dir kept: $WORK"
  else
    rm -rf "$WORK"
  fi
}
trap cleanup EXIT

fail() { echo "SMOKE FAILED: $*" >&2; exit 1; }

echo "=== building conflux-server + conflux-node ==="
(cd "$ROOT" && cargo build -p conflux-server -p conflux-node 2>&1 | tail -2)
SERVER="$ROOT/target/debug/conflux-server"
NODE="$ROOT/target/debug/conflux-node"
[ -x "$SERVER" ] || fail "conflux-server did not build"
[ -x "$NODE" ] || fail "conflux-node did not build"

echo "=== starting server (grpc :$GRPC_PORT, admin :$ADMIN_PORT) ==="
CONFLUX_TOPOLOGY=cross_device \
CONFLUX_MODE=research \
CONFLUX_AGGREGATOR=fedavg \
CONFLUX_QUORUM=1 \
CONFLUX_ROUND_TIMEOUT_SECS=30 \
CONFLUX_MIN_REPUTATION_SCORE=-1.0 \
CONFLUX_CLIP_NORM=1000 \
CONFLUX_NOISE_MULTIPLIER=0 \
CONFLUX_INITIAL_WEIGHTS_DIM="$DIM" \
CONFLUX_GRPC_ADDR="127.0.0.1:$GRPC_PORT" \
CONFLUX_HTTP_ADDR="127.0.0.1:$ADMIN_PORT" \
RUST_LOG=warn \
  "$SERVER" > "$WORK/server.log" 2>&1 &
SERVER_PID=$!
PIDS+=("$SERVER_PID")

# Alive AND answering AND answering as *us* — see run_demo.sh for the
# incident that made the third condition necessary.
healthy() {
  kill -0 "$SERVER_PID" 2>/dev/null || return 1
  curl -sf --max-time 2 "http://127.0.0.1:$ADMIN_PORT/health" 2>/dev/null \
    | grep -q '"status" *: *"ok"'
}
for _ in $(seq 1 80); do healthy && break; sleep 0.25; done
healthy || fail "server did not become healthy on :$ADMIN_PORT"
echo "server healthy"

echo "=== starting node (local hop :$LOCAL_PORT) ==="
CONFLUX_CLIENT_ID=smoke-node \
CONFLUX_LOCAL_ADDR="127.0.0.1:$LOCAL_PORT" \
CONFLUX_SERVER_ADDR="http://127.0.0.1:$GRPC_PORT" \
RUST_LOG=warn \
  "$NODE" > "$WORK/node.log" 2>&1 &
NODE_PID=$!
PIDS+=("$NODE_PID")
sleep 3
kill -0 "$NODE_PID" 2>/dev/null || fail "conflux-node exited during startup"

# `stub_client.py` does exactly one round per invocation, so N rounds is
# N invocations — which is closer to the real thing anyway: each one
# reconnects and re-registers, so a client that only works on a warm
# connection fails here.
echo "=== running the Python client for $ROUNDS round(s) ==="
for round in $(seq 1 "$ROUNDS"); do
  (cd "$HERE" && timeout 60 python3 stub_client.py \
    --address "127.0.0.1:$LOCAL_PORT" \
    --client-id "smoke-client-$round") >> "$WORK/client.log" 2>&1
  CLIENT_RC=$?
  [ "$CLIENT_RC" = "0" ] || { cat "$WORK/client.log"; fail "the Python client exited $CLIENT_RC on round $round"; }
  sleep 1
done
cat "$WORK/client.log"

# The checks that matter. A client can exit 0 having accomplished
# nothing: connect, register, then fail every submission and shrug.
grep -q "register: accepted=True" "$WORK/client.log" \
  || fail "the client never registered successfully"
grep -q "submit_delta: accepted=True" "$WORK/client.log" \
  || fail "the client never had a submission accepted"

# And the server's own answer, which the client cannot fake: the round
# counter has to have moved past 1, meaning a submission was actually
# aggregated into a checkpoint.
STATUS="$(curl -sf --max-time 3 "http://127.0.0.1:$ADMIN_PORT/round/status" 2>/dev/null)"
echo "server round status: $STATUS"
echo "$STATUS" | grep -qE '"round" *: *[2-9][0-9]*' \
  || fail "the server never advanced past round 1 — the client's updates were not aggregated"

kill -0 "$SERVER_PID" 2>/dev/null || fail "the server died during the run"

echo ""
echo "SMOKE PASSED — Python client completed $ROUNDS round(s) over the real hop"
