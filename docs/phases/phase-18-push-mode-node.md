# Phase 18 (draft) — Push mode in `conflux-node`

**Status: scoping draft, not started.**

## Scope

`conflux-net`'s server side already implements the push-mode RPC
(`service.rs`'s `SubscribeTasksStream`/`subscribe_tasks` — a real,
working streaming endpoint per spec's `FlTransport` service definition).
`conflux-node`'s client side does not: `bridge.rs`'s `NodeBridge::
subscribe_tasks` unconditionally returns `Err(DispatchError::Other("push
mode is not wired into conflux-node yet (Phase 6 scope: pull mode
only)"))`. Per spec §3's topology table, `cross_silo` is `push` + mTLS by
default — meaning **`conflux-node` cannot currently operate correctly in
Conflux's own default `cross_silo` configuration**, only by an operator
overriding `connection_mode` to `pull` against the topology's own
default. This phase closes that gap: the client-side half of a
capability the server has already had since Phase 3/5.

## Inputs

- `crates/conflux-net/src/service.rs` — the existing server-side
  `subscribe_tasks` implementation and its `TaskStream` type
  (`crates/conflux-net/src/dispatcher.rs`'s doc comment already
  describes it as "the stream type `subscribe_tasks` (push mode)
  returns"). This phase consumes that stream from the client side; no
  server-side change is needed.
- `crates/conflux-net/src/client.rs` — already has doc comments
  anticipating this gap directly (`"cross_silo is push + mTLS, but
  nothing stops a pull-mode..."`, `"cross_silo push mode is where mTLS
  actually applies"`) — read both comments in full before starting, they
  record design intent this brief inherits rather than re-deriving.
- `conflux-node::bridge.rs`'s existing `fetch_task`/retry-backoff pattern
  — push mode's failure handling (stream drop, reconnect) needs an
  analogous retry story, not the same code (a dropped stream isn't a
  single failed RPC call to retry, it's a subscription to re-establish).

## Design decision this brief makes explicit

Push mode changes `conflux-node`'s task-acquisition control flow from
"poll `fetch_task` on some cadence" to "hold one long-lived subscription,
react to whatever the server pushes." This isn't a drop-in replacement
for `fetch_task`'s retry loop — it needs its own control flow:

1. `NodeBridge` (or a new sibling type — see below) opens a
   `subscribe_tasks` stream at startup (or lazily, on first task need)
   and holds it open.
2. Each item the stream yields is a `TaskResponse`, handed to the same
   local-gRPC-to-Python path `fetch_task`'s result already goes through
   today — the *downstream* handling (local hop, training, privacy
   transform if Phase 17 lands first, submit) is unchanged regardless of
   which mode acquired the task.
3. **Stream lifecycle**: a dropped/errored stream triggers reconnect with
   the same exponential-backoff shape `fetch_task`/`submit_delta` already
   use (`MAX_ATTEMPTS`/`INITIAL_BACKOFF` constants in `bridge.rs`), not a
   new backoff policy — consistency with the existing retry conventions
   in the same file.

**Recommendation**: rather than making `NodeBridge::subscribe_tasks`
itself return a stream (awkward given `RoundDispatcher`'s trait shape is
built around discrete request/response calls, matching `fetch_task`'s
signature), introduce a small internal task-source abstraction inside
`conflux-node` — `PullTaskSource`/`PushTaskSource`, both yielding
`TaskResponse`s to the same downstream training loop — selected once at
startup based on `config.connection_mode.value`, rather than trying to
unify pull and push behind `RoundDispatcher::subscribe_tasks`'s existing
single-call signature (that method's real job, per `conflux-net`'s own
naming, is to hand back *a stream handle*, which is exactly what today's
`Err` return was withholding — this phase makes it return a real one).

## Deliverables

- `conflux-node::bridge.rs`: `NodeBridge::subscribe_tasks` returns a real
  `TaskStream` (from `upstream.subscribe_tasks(&self.node_client_id)`, the
  `PushTransport` equivalent of the existing `PullTransport` field) rather
  than the current hardcoded error — `NodeBridge` needs to hold either a
  `PullTransport` or a `PushTransport` upstream depending on
  `connection_mode`, likely an enum (`Upstream::Pull(PullTransport) |
  Upstream::Push(PushTransport)`) rather than two always-present fields.
- `conflux-node`'s main loop: branches on `connection_mode.value` at
  startup — `Pull` calls `fetch_task` on a timer/cadence (today's
  existing behavior, unchanged); `Push` calls `subscribe_tasks` once and
  processes the yielded stream, with reconnect-on-drop using the existing
  backoff constants.
- Reconnect logic: on stream error/end, log (ADR 0007 — a mode change
  this consequential should say so), back off, re-subscribe; a
  succession of reconnect failures past `MAX_ATTEMPTS` surfaces as a
  clear startup/runtime error, not a silent stall.

## Test plan

- Real end-to-end test, same rigor as Phase 6's pull-mode E2E test: a
  real server (via `conflux-net`'s existing `subscribe_tasks`
  implementation) and a real `conflux-node` in `Push` mode complete a
  full round — task delivered via the stream, trained (stub `ClientApp`),
  submitted back — without ever calling `fetch_task`.
- Reconnect test: server-side stream forcibly dropped mid-round (or
  before a task is sent); `conflux-node` reconnects and successfully
  receives the next pushed task, using the existing backoff constants
  (verify the backoff timing shape matches `fetch_task`'s retry test,
  for consistency, not just that it eventually succeeds).
- `cross_silo`'s topology default (`push` + mTLS) exercised together for
  the first time: real mTLS material (Phase 7e's `rcgen` test pattern)
  plus real push-mode delivery in one test, proving the two
  previously-separately-tested capabilities actually compose.
- Regression: every existing pull-mode test (Phase 6 onward) keeps
  passing completely unmodified — `Pull` mode's code path is untouched by
  this phase, only newly reachable alongside a working `Push` path.

## Definition of done

- [ ] `cargo test -p conflux-net -p conflux-node` passes, including the
      real push-mode E2E and reconnect tests.
- [ ] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [ ] `docs/STATUS.md`'s "Known deviations from spec" bullet updated;
      `cross_silo`'s default configuration is now fully functional
      end-to-end for the first time.
