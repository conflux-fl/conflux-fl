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
#       python3 -m _harness.trainer --model mlp \
#         --address 127.0.0.1:47100 --client-id site-7 --shard shard.pt --rounds 30
#
# (run from `baselines/`, or point at your own trainer — the node does not
# care what speaks to its loopback listener, only that something does.)
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

# Find the node, cheapest first. A client machine should not need this
# repository or a Rust toolchain: the released `cflux` binary carries the
# node as `cflux node start`, and `install.sh` puts it on PATH. Building
# from source is the developer case, so it is the last resort rather than
# the first — which is what this script used to assume.
#
# Both entry points read the same CONFLUX_* environment, so which one is
# found changes nothing below.
node_cmd=()
if command -v cflux >/dev/null 2>&1; then
  node_cmd=(cflux node start)
elif command -v conflux-node >/dev/null 2>&1; then
  node_cmd=(conflux-node)
else
  repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  node_bin="$repo_root/target/release/conflux-node"
  [ -x "$node_bin" ] || node_bin="$repo_root/target/debug/conflux-node"
  if [ ! -x "$node_bin" ]; then
    if [ ! -f "$repo_root/Cargo.toml" ]; then
      echo "error: no 'cflux' or 'conflux-node' on PATH, and this script is not \
inside a checkout to build one from." >&2
      echo "  install one:  curl -fsSL https://confluxfl.dev/install.sh | sh" >&2
      exit 1
    fi
    echo "no installed node found; building conflux-node (release) from source…"
    (cd "$repo_root" && cargo build --release -p conflux-node)
    node_bin="$repo_root/target/release/conflux-node"
  fi
  node_cmd=("$node_bin")
fi

node_pid=""
cleanup() { [ -n "$node_pid" ] && kill "$node_pid" 2>/dev/null || true; }
trap cleanup EXIT

echo "starting node (${node_cmd[*]}): id=$CONFLUX_CLIENT_ID mode=$CONFLUX_CONNECTION_MODE -> $CONFLUX_SERVER_ADDR"
"${node_cmd[@]}" &
node_pid=$!

# Wait for the node's loopback listener to accept connections before the
# trainer dials it — otherwise the trainer races the node's registration.
host="${CONFLUX_LOCAL_ADDR%:*}"
port="${CONFLUX_LOCAL_ADDR##*:}"
ready=""
for _ in $(seq 1 50); do
  kill -0 "$node_pid" 2>/dev/null || { echo "the node exited during startup" >&2; exit 1; }
  if (exec 3<>"/dev/tcp/$host/$port") 2>/dev/null; then
    exec 3>&- 3<&-
    ready=1
    break
  fi
  sleep 0.2
done
[ -n "$ready" ] || { echo "the node did not open $CONFLUX_LOCAL_ADDR in time" >&2; exit 1; }

echo "node ready; starting trainer: ${trainer[*]}"
"${trainer[@]}"
