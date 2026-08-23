# Phase 0 — Workspace scaffold + `conflux-proto`

## Scope
Stand up the Cargo workspace and all twelve crates from
`docs/spec/conflux-spec-v1.md` §2 as empty, compiling scaffolds with the
dependency graph wired via path deps (§2's acyclic graph: `conflux-proto` and
`conflux-config` at the bottom; `conflux-net`/`conflux-buffer`/`conflux-core`
depend on `conflux-proto`; `conflux-server` integrates the library crates;
`conflux-node` depends only on `conflux-proto` and `conflux-net`). Design and
implement the `conflux-proto` `.proto` schema (§3) and its Rust codegen.
This phase explicitly does **not** implement any business logic in any other
crate — leaf crates (`conflux-config`, `conflux-registry`, etc.) are Phase 1
onward's territory (§10).

## Inputs (what must already exist)
Nothing — this is the first phase (see plan `docs/spec/conflux-development-plan.md` §4, row S0).

Relevant ADRs: [0009-project-name-conflux](../adr/0009-project-name-conflux.md).

## Deliverables
- Root `Cargo.toml` workspace with all twelve crates as members.
- `crates/conflux-proto/` — the `FlTransport` service definition from spec
  §3:
  ```protobuf
  service FlTransport {
    rpc FetchTask (FetchTaskRequest) returns (TaskResponse);
    rpc SubscribeTasks (SubscribeRequest) returns (stream TaskResponse);
    rpc SubmitDelta (stream DeltaChunk) returns (SubmitAck);
    rpc Register (RegisterRequest) returns (RegisterResponse);
    rpc Heartbeat (HeartbeatRequest) returns (HeartbeatResponse);
  }
  ```
  plus the message types it references (`FetchTaskRequest`, `TaskResponse`,
  `SubscribeRequest`, `DeltaChunk`, `SubmitAck`, `RegisterRequest`,
  `RegisterResponse`, `HeartbeatRequest`, `HeartbeatResponse`), generated Rust
  bindings, and re-exports other crates can depend on.
- `python/conflux_client/` placeholder directory (SDK design deferred, see
  [0005-python-sdk-deferred](../adr/0005-python-sdk-deferred.md)).
- Empty `crates/conflux-{config,registry,store,selector,net,buffer,privacy,reputation,core}`
  library crates and `crates/conflux-{server,node}` binary crates, each
  compiling with only their spec §2 one-line purpose as a module doc comment.

## Test plan
- `cargo build --workspace` succeeds with zero warnings.
- `conflux-proto`'s generated types round-trip through a basic
  encode/decode test for at least one message (e.g. `RegisterRequest`).
- Confirm the dependency graph has no cycles (`cargo tree` per crate matches
  §2's described graph).

## Definition of done
- [x] `cargo new`'d workspace with all twelve crates present under `crates/`.
- [x] Path dependencies match §2's dependency graph exactly.
- [x] `conflux-proto` schema committed and codegen wired into the build.
- [x] `python/conflux_client/` exists with a README noting the deferred SDK
      design ([0005](../adr/0005-python-sdk-deferred.md)).
- [x] `docs/STATUS.md` updated to mark Phase 0 done and Phase 1 next.
- [x] Open Item 1 from spec §11 resolved (crates.io namespace availability
      for `conflux`), or explicitly logged as still open in `docs/STATUS.md`.
