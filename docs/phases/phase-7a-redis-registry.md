# Phase 7a — `RedisRegistry`

## Scope
A second `conflux-registry::Registry` implementation backed by Redis —
durable across restarts and shared across multiple `conflux-server`
processes, unlike `InMemoryRegistry` (Phase 1). Same trait, same crate,
new backend — this is exactly the swap ADR 0003 (no multi-tenancy) and the
`Registry` trait were designed for: `conflux-server`'s `AppState` picks a
backend, the round pipeline doesn't care which.

**Does not build**: wiring `RedisRegistry` into `conflux-server`'s
`AppState` as a config-selectable option (that's `conflux-config`'s
strategy-registry-by-name work, still deferred per Phase 5/6's notes) —
this phase ships the implementation and proves it works against a real
Redis, not the integration point.

## Inputs
- `conflux-registry::{Registry, ClientId, ClientInfo, RegistryError}`
  (Phase 1) — the exact trait this must implement.
- A real Redis for testing (this session: Docker, `redis:7-alpine`,
  isolated container `conflux-dev-redis` on `127.0.0.1:16379`, not the
  default port — doesn't collide with any other Redis instance the host
  might have).

## Deliverables
- `RedisRegistry` implementing `Registry`: `register`, `heartbeat`,
  `evict_expired`, `active_clients`, backed by a Redis hash (client id →
  last-heartbeat timestamp) plus Redis's own key `EXPIRE` semantics, so
  eviction gets to be "let Redis do it" rather than a separate sweep.
- Connection config via a plain `redis://` URL string, not
  `conflux-config`-driven (matches Phase 5/6's precedent — `main.rs`-level
  wiring stays env-var/argument based until Open Item 2 gets resolved).

## Test plan
- Real integration tests against a live Redis (not mocked): register →
  appears in `active_clients`; duplicate registration errors; heartbeat on
  an unknown client errors; a TTL-based expiry actually removes a client
  from `active_clients` after the TTL elapses (short TTL, e.g. 200ms, so
  the test doesn't sleep long); two independent `RedisRegistry` handles
  pointed at the same Redis see each other's writes — the property
  `InMemoryRegistry` structurally cannot have, and the actual reason this
  backend exists.

## Definition of done
- [x] `cargo test -p conflux-registry` passes against a real Redis
      (documented setup command in the test file or this brief).
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] `docs/STATUS.md` updated.

## Deviation discovered while implementing
`Registry`'s trait methods were synchronous (Phase 1) — fine for
`InMemoryRegistry`, impossible for a backend that does real network I/O.
Converted the whole trait to native `async fn` (stable since Rust 1.75, no
`async-trait` needed since nothing needs `dyn Registry`); also made
`active_clients` fallible (`Result<Vec<ClientId>, RegistryError>`) since a
Redis outage returning as an empty `Vec` would be indistinguishable from a
genuinely idle experiment. `evict_expired` stayed infallible — see its
updated doc comment for why. Added `RegistryError::Backend(String)` for
real backend failures Phase 1 never needed to represent.
