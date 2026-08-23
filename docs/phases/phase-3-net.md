# Phase 3 — `conflux-net`

## Scope
Dual-mode gRPC transport over `conflux-proto`'s `FlTransport` service:
`PullTransport`/`PushTransport` (client side, spec §10's naming) and a
generic server-side adapter that turns an injected dispatcher into a real
tonic service. Does **not** build the dispatcher's actual business logic —
that's `conflux-registry`/`conflux-selector`/`conflux-buffer` wired
together in `conflux-server` (Phase 5). This phase only builds and tests
the transport mechanics themselves, using a trivial in-memory dispatcher
for its own integration test. Also does not build mTLS/JWT auth
enforcement (topology-dependent per spec §3; deferred to whichever phase
first needs a real auth story — likely Phase 5/7) or retry/backoff (that's
`conflux-node`'s job per spec §7, Phase 6).

## Inputs (what must already exist)
- `conflux-proto`'s generated `fl_transport_client`/`fl_transport_server`
  modules (Phase 0, already built) — the exact client method signatures
  and the server `FlTransport` trait (including its associated
  `SubscribeTasksStream` type) were inspected directly from the generated
  code at `target/debug/build/conflux-proto-*/out/conflux.v1.rs` before
  writing this brief; re-generate and re-check if `conflux-proto`'s
  `.proto` changes.
- Spec §3's table: `cross_silo` uses push/mTLS; `cross_device`,
  `crowdsource`, `edge` use pull/JWT.
- Spec §2's dependency graph: `conflux-net` depends only on
  `conflux-proto`.

## Deliverables
- `TransportError` (thiserror) wrapping `tonic::transport::Error` and
  `tonic::Status` — the client-side error type.
- `DispatchError` (thiserror) — the error type a `RoundDispatcher`
  implementation (built in Phase 5) returns; mapped to a `tonic::Status` at
  the service boundary, not leaked as a raw string.
- `RoundDispatcher` trait: `fetch_task`, `subscribe_tasks`, `submit_delta`,
  `register`, `heartbeat` — the seam between conflux-net's transport
  mechanics and whatever crate ends up answering each call for real
  (`conflux-server`, Phase 5).
- `FlTransportService<D: RoundDispatcher>` — implements the generated
  `fl_transport_server::FlTransport` trait, delegating every RPC to an
  injected `Arc<D>`. Collects `submit_delta`'s streamed `DeltaChunk`s into
  a `Vec` before handing them to the dispatcher (Phase 3 doesn't need
  incremental/backpressured streaming into the dispatcher itself).
- `PullTransport` / `PushTransport` — thin client-side wrappers around the
  generated `FlTransportClient<Channel>`, each with `connect`, `register`,
  `heartbeat`, `submit_delta`, plus `PullTransport::fetch_task` /
  `PushTransport::subscribe_tasks` respectively.

## Test plan
- A real integration test: bind a TCP listener on `127.0.0.1:0`, serve
  `FlTransportServer::new(FlTransportService::new(Arc::new(test_dispatcher)))`
  on it via `tokio::spawn`, connect a real `PullTransport`/`PushTransport`
  to the assigned port, and exercise `register`, `heartbeat`,
  `fetch_task`/`subscribe_tasks`, and `submit_delta` end-to-end over actual
  gRPC — not just prost encode/decode (Phase 0 already covers that).
- An unknown-client `DispatchError` maps to the right `tonic::Status` code
  at the client, not a generic "internal error".

## Definition of done
- [x] `cargo test -p conflux-net` passes, including the real over-the-wire
      integration test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.
