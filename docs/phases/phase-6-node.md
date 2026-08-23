# Phase 6 — `conflux-node` + stub Python client

## Scope
`conflux-node`: registers with the real `conflux-server` over the network
(spec §7), then runs a *local* gRPC server on loopback — the exact same
`FlTransport` service definition, reused as ADR 0004 intends — for a
Python `ClientApp` to connect to as *its* client. `conflux-node` bridges
every local-hop call to the real upstream server: a local `FetchTask`
pulls the real task from `conflux-server`; a local `SubmitDelta` forwards
the trained delta upstream. Retry/backoff wraps the upstream calls (spec
§7's job for this crate).

Alongside it: a real stub Python `ClientApp`
(`python/conflux_client/stub_client.py`) — fixed dummy weights, no
PyTorch — that proves the local hop actually works across the language
boundary, not just within Rust.

**Does not build**: the real Python SDK or model-distribution mechanism
(ADR 0005, still deferred), client-side privacy transform (spec §7 marks
it *optional*, and it would require `conflux-node` to depend on
`conflux-privacy` — a new edge not in spec §2's stated graph; deferred
until the feature is actually needed), auth token refresh (no real auth
exists yet, matching Phase 3's deferral), or push mode (spec §10 scopes
Phase 6's end-to-end test to pull mode only — `conflux-node` only wires a
`PullTransport` upstream).

## Inputs (what must already exist)
- Spec §7's architecture diagram and role split: `conflux-node` owns
  registration/heartbeat, task fetch, retry/backoff, and running the local
  server; the local hop reuses `TaskResponse`/`DeltaChunk` verbatim (ADR
  0004).
- `conflux-net`'s `PullTransport` (Phase 3) — what `conflux-node` uses to
  talk to the real server — and `FlTransportService<D: RoundDispatcher>`
  (Phase 3) — reused as-is to serve the local hop, with `conflux-node`
  supplying the `RoundDispatcher` impl instead of writing a new service
  adapter.
- `conflux-server`'s real dispatcher (Phase 5) — what `conflux-node`
  connects to on the network side; Phase 5's own integration test already
  previewed this exact interaction using `PullTransport` as a stand-in for
  `conflux-node`.
- Spec §2's dependency graph: `conflux-node` depends only on
  `conflux-proto` and `conflux-net` — no `conflux-config`, no
  `conflux-privacy`. CLI/config resolution stays env-var-based, matching
  `conflux-server`'s Phase 5 `main.rs`.

## Deliverables
- `NodeBridge`: implements `conflux-net::RoundDispatcher` for the local
  hop. `register`/`heartbeat` are answered locally without touching the
  network (`conflux-node` already registered itself with the real server
  at startup — spec §7 doesn't ask the local Python side to be tracked as
  a separate lifecycle entity). `fetch_task`/`submit_delta` forward to a
  `Mutex<PullTransport>` pointing at the real server, each wrapped in a
  small retry-with-backoff loop.
- `conflux-node`'s `main.rs`: connects + registers upstream, binds the
  local listener, serves `FlTransportServer::new(FlTransportService::new(
  Arc::new(NodeBridge)))` on it — reusing Phase 3's adapter directly, no
  new service-wiring code.
- `python/conflux_client/stub_client.py`: connects to `conflux-node`'s
  local address, `Register`s, `FetchTask`s, "trains" by adding a fixed
  offset to every weight (no PyTorch), `SubmitDelta`s the result. Plus
  `generate_proto.sh` (regenerates `*_pb2.py`/`*_pb2_grpc.py` from
  `conflux-proto`'s `.proto` via `grpc_tools.protoc` — generated files
  aren't committed, same as not committing `target/`) and
  `requirements.txt`.

## Test plan
- Hermetic `cargo test` (no Python involved — that's the manual smoke test
  below): a fake upstream `RoundDispatcher` (same pattern as Phase 3/5's
  test dispatchers) stands in for `conflux-server`; a real `PullTransport`
  stands in for the Python `ClientApp`, connecting to `conflux-node`'s real
  local server. Fetching a task and submitting a delta through the local
  hop is asserted to reach the fake upstream unchanged — proving
  `NodeBridge`'s forwarding logic, not just that two independent halves
  compile.
- A flaky fake-upstream dispatcher (fails N times, then succeeds) proves
  `NodeBridge`'s retry/backoff actually recovers rather than failing the
  first RPC.
- **Manual, real cross-process, cross-language smoke test** (not part of
  `cargo test` — spawning arbitrary external processes inside automated
  Rust tests is fragile and not standard practice): run `conflux-server`,
  `conflux-node`, and the real `stub_client.py` together and confirm a
  checkpoint lands, the same rigor as Phase 5's binary smoke test. Document
  the exact commands and observed output in `docs/STATUS.md`.

## Definition of done
- [x] `cargo test -p conflux-node` passes, including the retry test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] The manual three-process smoke test actually run once, with its
      output recorded in `docs/STATUS.md` (not just claimed).
- [x] `docs/STATUS.md` updated, including what's explicitly deferred above.
