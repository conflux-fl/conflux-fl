#!/usr/bin/env bash
# Regenerates fl_transport_pb2.py / fl_transport_pb2_grpc.py from
# conflux-proto's .proto — the same schema the Rust side uses (ADR 0004).
# Generated files aren't committed (same reasoning as not committing
# target/); run this before running stub_client.py.
set -euo pipefail
cd "$(dirname "$0")"

python3 -m grpc_tools.protoc \
  -I ../../crates/conflux-proto/proto \
  --python_out=. \
  --grpc_python_out=. \
  ../../crates/conflux-proto/proto/fl_transport.proto
