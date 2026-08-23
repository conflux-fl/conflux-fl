# 0004 — Client/server split via local gRPC

## Context
Conflux's design keeps Python (PyTorch) entirely client-side for model
training, while Rust owns networking, orchestration, aggregation, privacy,
and reputation (spec §1). This requires a clean, well-typed handoff between
the Rust client process and the Python training process running alongside
it, without duplicating the wire schema.

## Decision
`conflux-node` (Rust) owns registration, heartbeat, auth token refresh, task
fetch/receive, retry/backoff, and optional client-side privacy transform. It
hands the actual training task to a Python `ClientApp` over a **local
loopback gRPC channel**, reusing the exact same `.proto` schema
(`TaskResponse`/`DeltaChunk`) used for the network hop to the server — no
TLS, localhost only (spec §7).

## Consequences
- One `.proto` schema serves three hops: server↔node (network) and
  node↔Python (loopback) — see spec §3.
- Step 2 (local training) is the only FL step with zero Rust-side
  algorithmic logic, enforcing the boundary that keeps PyTorch/GPU training
  out of the Rust codebase (spec §8).
- The Python side has no network exposure of its own; all external
  connectivity concerns live in `conflux-node`.
