#!/usr/bin/env bash
# Shared setup for every experiment script in this directory — build the
# runner once (release mode; these scripts invoke it many times per run,
# and a debug build's per-call overhead adds up across a real sweep),
# then expose $RUNNER as the compiled binary path.
set -euo pipefail

SCRIPT_DIR_LIB="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR_LIB/../../.." && pwd)"
RESULTS_DIR="$REPO_ROOT/docs/research/results"
mkdir -p "$RESULTS_DIR"

echo "=== building conflux-attacks' run_experiment example (release) ===" >&2
(cd "$REPO_ROOT" && cargo build --release -p conflux-attacks --example run_experiment 2>&1 | tail -5) >&2
RUNNER="$REPO_ROOT/target/release/examples/run_experiment"
