# Phase 4a — `conflux-buffer`

## Scope
Async staging for one round's incoming client updates: collect
`conflux_proto::ClientDelta`s as they arrive, and close the batch on
whichever happens first — quorum reached or a timeout — logging which one
it was (ADR 0007). Does **not** build `DeltaChunk` reassembly (spec §8's
sequence diagram shows `Net->>Buf: push(delta)` with an already-assembled
delta, so reassembly is upstream, presumably `conflux-server`'s job in
Phase 5) and does not decide *when* a round starts or what happens to the
flushed batch next (privacy transform, reputation filtering, aggregation —
all downstream, Phase 5's round loop).

## Inputs (what must already exist)
- Spec §8 sequence diagram: `Net->>Buf: push(delta)` then
  `Buf-->>Server: batch ready (quorum or timeout — logged either way)`.
- ADR [0007](../adr/0007-explainable-config-resolution.md): "`conflux-buffer`
  logs whether a round closed on quorum or timeout" — same explainability
  principle as config resolution, applied to a runtime decision instead of
  a startup one.
- Spec §2's dependency graph: `conflux-buffer` depends on `conflux-proto`
  (it operates on `ClientDelta` directly, unlike Phase 2's leaf crates).
- `conflux-proto::ClientDelta`'s fields (Phase 0):
  `client_id: String`, `round: u64`, `weights: Vec<u8>` (little-endian
  packed `f32`), `num_samples: u64`.

## Deliverables
- `BufferError` (thiserror): pushing a delta whose `round` doesn't match
  the buffer's round is an error, not silently accepted or dropped.
- `FlushReason` enum: `Quorum | Timeout`.
- `FlushResult`: `{ round: u64, reason: FlushReason, deltas: Vec<ClientDelta> }`.
- `RoundBuffer::new(round: u64, quorum: usize)`, `push(&self, delta:
  ClientDelta) -> Result<(), BufferError>`, and `async fn
  await_flush(&self, timeout: Duration) -> FlushResult` — races quorum
  against the timeout using `tokio::sync::Notify` (woken on every `push`)
  rather than polling, and logs the outcome (`eprintln!`, matching
  `conflux-reputation`'s rejection-logging style) before returning.

## Test plan
- Quorum reached before timeout: `await_flush` returns
  `FlushReason::Quorum` with exactly the pushed deltas, well before the
  timeout elapses (assert on wall-clock elapsed time, not just the
  returned reason).
- Timeout elapses before quorum: `await_flush` returns
  `FlushReason::Timeout` with whatever was collected (including zero
  pushes).
- Pushing a delta for the wrong round returns `BufferError` and does not
  affect the batch.
- Concurrent pushes from several `tokio::spawn`ed tasks racing to reach
  quorum — validates the `Notify`-based wakeup isn't losing pushes under
  real concurrency, not just sequential calls.

## Definition of done
- [x] `cargo test -p conflux-buffer` passes, including the concurrent-push
      test.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.
