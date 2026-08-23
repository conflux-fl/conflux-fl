# Phase 7f — `S3Store`

## Scope
A third `conflux-store::Store` backend against object storage, per spec
§10's Phase 7 list. Same append-only-checkpoint shape as `PostgresStore`
(Phase 7b) — one object per round, `load_latest_weights` picks the highest
round number found — but for a deployment that already standardizes on S3
(or an S3-compatible service) rather than running its own Postgres just
for checkpoints.

**Test target decision**: MinIO (Docker, `conflux-dev-minio`, ports 19000
API / 19001 console), not real AWS S3 — matches every other Phase 7
backend's approach (real infra, but self-hosted and disposable, not a
cloud account this session doesn't have and shouldn't assume). MinIO
implements the S3 API closely enough that `aws-sdk-s3` (the official,
standard crate) works against it unmodified via a custom endpoint URL —
so the implementation is genuinely S3-compatible, not MinIO-specific.

## Inputs
- `conflux-store::{Store, StoreError}` (Phase 2a) — the exact trait this
  must implement, same as `FileStore`/`PostgresStore` before it.
- `conflux-store::FileStore`'s round-number-scanning approach (Phase 2a):
  `S3Store` uses the same idea — list objects under a prefix, parse round
  numbers out of the keys, pick the max — rather than needing a separate
  index object.

## Deliverables
- `S3Store::connect(endpoint_url, bucket, access_key, secret_key) ->
  Result<Self, StoreError>` — configures `aws-sdk-s3` with a custom
  endpoint (so it works against MinIO/any S3-compatible service, not just
  real AWS) and path-style addressing (required by most self-hosted S3
  implementations, MinIO included).
- Object key scheme: `checkpoint-<round>.bin` (mirrors `FileStore`'s
  on-disk naming exactly) under an optional key prefix, so multiple
  `S3Store` instances can share one bucket the same way
  `PostgresStore`/`RedisRegistry` share one database/Redis under different
  tables/key namespaces.
- `load_latest_weights`: list objects under the prefix, parse round
  numbers from keys, `GetObject` on the highest one.
- `save_checkpoint`: `PutObject` the little-endian `f32` bytes (same wire
  convention as every other checkpoint backend) — S3 `PutObject` is
  natively an overwrite-if-exists operation, so a retried round (the
  Phase 6 buffer race) overwrites for free, no explicit upsert logic
  needed (unlike `PostgresStore`'s `ON CONFLICT`).

## Test plan
- Real integration tests against live MinIO (not mocked): save then load
  round-trips actual bytes through the object store; loading the highest
  round when multiple checkpoints exist; loading with no checkpoint
  returns `StoreError::NoCheckpoint`; saving the same round twice
  overwrites rather than erroring (proving the "no upsert logic needed"
  claim above is actually true, not just assumed).
- Per-test key-prefix isolation, same fix as 7a/7b/7d: `cargo test`'s
  parallel execution against one real, never-wiped bucket needs it.

## Definition of done
- [x] `cargo test -p conflux-store` passes against real MinIO.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.

## Real cross-crate conflict found and fixed
`aws-sdk-s3` pulls in rustls with the `aws-lc-rs` crypto provider.
`conflux-net`'s Phase 7e mTLS work had already picked `tls-ring` (the
`ring` provider). Both compiled fine in isolation, but `cargo test
--workspace` unifies feature flags across the whole workspace's shared
`Cargo.lock`, so `conflux-net`'s own test binaries ended up with *both*
crypto providers linked in — and rustls panics at runtime
("Could not automatically determine the process-level CryptoProvider")
because it won't guess between two. `cargo test -p conflux-net` alone
never surfaced this (that invocation's build plan excludes `conflux-store`
and never pulls in `aws-lc-rs` at all) — only `--workspace` did. Fixed by
switching `conflux-net` to `tls-aws-lc`, matching the AWS SDK's provider
so only one is ever linked in. See `crates/conflux-net/Cargo.toml`'s
comment on the `tonic` dependency.
