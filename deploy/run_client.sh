#!/usr/bin/env bash
# Launch ONE Conflux FL client on this machine: a `conflux-node` bridge plus
# a trainer that talks to it over the local loopback. One command per
# participant — the multi-host equivalent of a single column in run_demo.sh.
#
# The trainer command goes after `--`; everything before it configures the
# node via the CONFLUX_* env below (all optional except the client id).
#
#   CONFLUX_CLIENT_ID=site-7 \
#   CONFLUX_SERVER_ADDR=http://fl.example.org:50051 \
#     deploy/run_client.sh -- \
#       python3 python/conflux_client/examples/e2e_pytorch_mnist/trainer_client.py \
#         --address 127.0.0.1:47100 --client-id site-7 --shard shard.pt --rounds 30
#
# The trainer must point at the node's loopback listener, i.e. the same
# host:port as CONFLUX_LOCAL_ADDR (default 127.0.0.1:47100).
#
# Auth/TLS are read by conflux-node directly from the environment, so just
# export them before calling this:
#   CONFLUX_NODE_AUTH_TOKEN                          per-client token / JWT
#   CONFLUX_TLS_SERVER_CA_PATH + CONFLUX_TLS_DOMAIN  server-authenticated TLS
#   + CONFLUX_TLS_CLIENT_CERT_PATH + _KEY_PATH       mutual TLS
set -euo pipefail

: "${CONFLUX_CLIENT_ID:?set CONFLUX_CLIENT_ID (a unique id for this client)}"
export CONFLUX_SERVER_ADDR="${CONFLUX_SERVER_ADDR:-http://127.0.0.1:50051}"
export CONFLUX_LOCAL_ADDR="${CONFLUX_LOCAL_ADDR:-127.0.0.1:47100}"
export CONFLUX_CONNECTION_MODE="${CONFLUX_CONNECTION_MODE:-pull}"
export CONFLUX_MODE="${CONFLUX_MODE:-production}"
export CONFLUX_CLIENT_APP_KIND="${CONFLUX_CLIENT_APP_KIND:-real}"

# The trainer command is everything after `--`.
trainer=()
seen_sep=0
for arg in "$@"; do
  if [ "$seen_sep" = 1 ]; then
    trainer+=("$arg")
  elif [ "$arg" = "--" ]; then
    seen_sep=1
  fi
done
if [ "${#trainer[@]}" -eq 0 ]; then
  echo "error: give the trainer command after '--' (see the header of this script)" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
node_bin="$repo_root/target/release/conflux-node"
[ -x "$node_bin" ] || node_bin="$repo_root/target/debug/conflux-node"
if [ ! -x "$node_bin" ]; then
  echo "building conflux-node (release)…"
  (cd "$repo_root" && cargo build --release -p conflux-node)
  node_bin="$repo_root/target/release/conflux-node"
fi

node_pid=""
cleanup() { [ -n "$node_pid" ] && kill "$node_pid" 2>/dev/null || true; }
trap cleanup EXIT

echo "starting conflux-node: id=$CONFLUX_CLIENT_ID mode=$CONFLUX_CONNECTION_MODE -> $CONFLUX_SERVER_ADDR"
"$node_bin" &
node_pid=$!

# Wait for the node's loopback listener to accept connections before the
# trainer dials it — otherwise the trainer races the node's registration.
host="${CONFLUX_LOCAL_ADDR%:*}"
port="${CONFLUX_LOCAL_ADDR##*:}"
ready=""
for _ in $(seq 1 50); do
  kill -0 "$node_pid" 2>/dev/null || { echo "conflux-node exited during startup" >&2; exit 1; }
  if (exec 3<>"/dev/tcp/$host/$port") 2>/dev/null; then
    exec 3>&- 3<&-
    ready=1
    break
  fi
  sleep 0.2
done
[ -n "$ready" ] || { echo "conflux-node did not open $CONFLUX_LOCAL_ADDR in time" >&2; exit 1; }

echo "node ready; starting trainer: ${trainer[*]}"
"${trainer[@]}"
