# Phase 2a — `conflux-store`

## Scope
Model checkpoint persistence: load the latest global model weights, save a
new checkpoint after each round. Ships two backends — `InMemoryStore`
(research/testing) and `FileStore` (one flat file per round on disk).
Does **not** build: `S3Store` (Phase 7), or a full experiment-metadata
schema beyond round number + weights — the spec doesn't elaborate what
"experiment metadata" contains beyond what the sequence diagram in §8
actually uses (`load_latest_weights`, `save_checkpoint(round, weights)`), so
this phase keeps to that and leaves richer metadata for whenever a concrete
need shows up.

## Inputs
- Sequence diagram, spec §8: `Store.load_latest_weights() -> global_weights`,
  `Store.save_checkpoint(round, new_weights)`.
- Step table, spec §8: Step 0 (initialize) and Step 4 (aggregate → save
  checkpoint) both route through `conflux-store`.
- No cross-crate type dependency — spec §2's dependency graph doesn't list
  `conflux-store` depending on `conflux-proto` or any other crate, so this
  phase works in plain `Vec<f32>`, not `conflux-proto::TaskResponse`.

## Deliverables
- `Store` trait: `load_latest_weights() -> Result<Vec<f32>, StoreError>`,
  `save_checkpoint(round: u64, weights: &[f32]) -> Result<(), StoreError>`.
- `InMemoryStore` — holds the latest `(round, weights)` behind a mutex,
  seeded with an initial global model at construction.
- `FileStore` — one file per round under a configured directory
  (`checkpoint-<round>.bin`, little-endian `f32` array);
  `load_latest_weights` picks the highest round number found on disk.

## Test plan
- `InMemoryStore`: save then load round-trips; load before any save returns
  the seeded initial model.
- `FileStore`: save then load round-trips through actual file I/O (temp
  dir); loading picks the highest round when multiple checkpoints exist;
  loading an empty directory errors clearly rather than panicking.

## Definition of done
- [x] `cargo test -p conflux-store` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.
