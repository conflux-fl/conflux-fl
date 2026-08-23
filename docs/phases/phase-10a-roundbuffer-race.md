# Phase 10a — Closing the `RoundBuffer` lost-update race

## Scope
Closes the "residual `RoundBuffer` race" flagged since Phase 6/7d
(`docs/phases/phase-7g-load-testing.md`, `docs/STATUS.md`) — evidence it
doesn't manifest at 30-client load-test scale (7g) was never proof it's
closed, because that test never drove the retry path that actually
triggers it.

**The mechanism** (see `docs/phases/phase-7g-load-testing.md` and this
phase's own investigation): `run_round` (`conflux-server/src/round.rs`)
creates a *new* `RoundBuffer` each call and swaps it into
`AppState.current_buffer`. On `AggregatorError::EmptyBatch` (zero
submissions, or all submissions failed the reputation filter),
`main.rs`'s round loop retries `run_round` for the *same* round number
without ever having advanced `state.round` or cleared `current_buffer` on
the error path. Between attempt A's `await_flush` returning (its snapshot
already taken and handed to the now-failed aggregation) and attempt B
installing buffer B, `current_buffer` still points at buffer A. A
`submit_delta` landing in that window pushes into buffer A —
`RoundBuffer::push` has no concept of "already flushed," only "right
round number," and the round number hasn't changed across the retry — so
the push is accepted (`SubmitAck{accepted:true}`) but never read again.
A client is told its submission succeeded; it silently never counts.

## Inputs
- `crates/conflux-buffer/src/lib.rs` — `RoundBuffer::push`/`await_flush`,
  the two operations that race.
- `crates/conflux-server/src/round.rs` — `run_round`'s error path, which
  is what makes the race window reachable via a real retry rather than a
  purely theoretical interleaving.
- `docs/phases/phase-7g-load-testing.md` — the original description of
  the race window this phase closes.

## Deliverables
- `RoundBuffer` gains a `closed: AtomicBool` field (`Ordering::Release`
  on write, `Ordering::Acquire` on read — matches the existing `Mutex`'s
  happens-before guarantees, no weaker). Set `true` immediately before
  constructing the returned `FlushResult` in *both* branches of
  `await_flush` (quorum and timeout) — a buffer whose snapshot has been
  taken can never accept another push, full stop, regardless of
  `current_buffer`'s swap timing.
- `push` checks `closed` first and returns a new `BufferError::Closed`
  variant instead of silently accepting — turns the silent lost-update
  into an explicit, actionable error.
- `conflux-server::dispatcher.rs`'s `submit_delta` maps
  `BufferError::Closed` to a distinct `DispatchError` (not lumped into
  the generic `Other` variant) so a client can tell "this round already
  closed, re-`fetch_task`" apart from a genuine backend failure.

## Test plan
- Unit test: `push` after `await_flush` has already returned (either
  flush reason) returns `BufferError::Closed`, not a silently-accepted
  push.
- Real concurrency test reproducing the actual race window: spawn
  `await_flush` and, once it's resolved (quorum met immediately), spawn a
  concurrent `push` for the same round — assert it errors rather than
  succeeding into a buffer nobody will ever read again.
- Every pre-existing `conflux-buffer` test (Phase 4/7g) still passes
  unmodified.
- `conflux-server` integration test: after a round retries past
  `EmptyBatch` once, a late submission against the *old* round number
  buffer is rejected with the new distinct error, not silently swallowed.

## Definition of done
- [x] `cargo test -p conflux-buffer -p conflux-server` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated — the race marked closed, not just
      evidenced-absent.

## Outcome

Implemented slightly differently from the brief's literal `AtomicBool`
suggestion, on review: a separate atomic flag beside the `Mutex` still
leaves a TOCTOU (a push can pass the flag check, then race the mutex lock
against the flush taking its snapshot). Instead, `RoundBuffer.deltas:
Mutex<Vec<ClientDelta>>` became `state: Mutex<BufferState>` where
`BufferState::Open(Vec<ClientDelta>) | Closed` — "closed" lives *inside*
the same mutex as the deltas, so closing is atomic with taking the
snapshot by construction: a `push` acquiring the lock either lands before
the snapshot (correctly included) or observes `Closed` already (errors),
with no window in between. `BufferError::Closed` is the new variant;
`conflux-net::DispatchError::RoundClosed` (→
`Status::failed_precondition`) is the distinct client-facing error
`conflux-server::dispatcher.rs`'s `submit_delta` maps it to.

Tests: `push_after_flush_errors_...` and `push_after_timeout_flush_also_errors`
(unit-level, both flush reasons); `racing_push_against_quorum_flush_never_silently_loses_a_delta`
(200 iterations, multi-threaded, directly reproduces the interleaving —
every push either lands in the batch or is explicitly rejected, never
neither); every pre-existing `conflux-buffer` test passes unmodified.
`crates/conflux-server/tests/round_buffer_race.rs` reproduces the actual
`run_round` retry precondition end-to-end: zero active clients →
`EmptyBatch` → `current_buffer` still points at the closed round-1
buffer → a late `submit_delta` for round 1 gets `DispatchError::
RoundClosed`, not a false `accepted: true`.

145 tests passing workspace-wide (was 142 at the end of Phase 9), stable
across 3 repeated runs; `cargo fmt --check` and
`cargo clippy --workspace --all-targets` both clean.
