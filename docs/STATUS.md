# Conflux — Status

Last updated: 2026-08-31 — **stabilization Tiers 1–5 complete**. Three remotely-triggerable defects fixed, the admin API authenticated, the project made releasable (Apache-2.0, workspace-inherited metadata, declared MSRVs, a compose file, evnx-managed env config, CI), the public API documented, reviewed, and demonstrated, and the three production-hardening defects a post-Tier-4 audit found now closed. 387 tests, clippy clean under `-D warnings`, fmt clean. Version 0.2.0.

## Done
- [x] Git repo initialized
- [x] Cargo workspace scaffolded — all twelve crates from spec §2, path
      dependencies matching §2's dependency graph
- [x] `python/conflux_client/` — stub Python `ClientApp` (Phase 6)
- [x] Nine ADRs written under `docs/adr/`
- [x] Phase 0: `conflux-proto`'s `.proto` schema + codegen.
- [x] Phase 1: `conflux-config` (six-tier resolution, provenance logging)
      + `conflux-registry` (`Registry` trait, `InMemoryRegistry`).
- [x] Phase 2: the four leaf crates — `conflux-store`, `conflux-selector`,
      `conflux-privacy`, `conflux-reputation`.
- [x] Phase 3: `conflux-net`'s dual-mode gRPC transport.
- [x] Phase 4: `conflux-buffer` + `conflux-core` (`FedAvg`, `robust`
      family scaffold).
- [x] Phase 5: `conflux-server` — `AppState`, real `RoundDispatcher`,
      `run_round`'s full pipeline, HTTP admin surface.
- [x] Phase 6: `conflux-node` + a real stub Python `ClientApp`, verified
      with an actual three-process, cross-language smoke test.
- [x] **Phase 7 complete, all seven sub-phases (7a–7g)**:
  - **7a — `RedisRegistry`**: `Registry` backend on a real Redis. Required
    converting `Registry` to `async fn` (native syntax) since Phase 1's
    synchronous design only had to work for an in-process `HashMap`. Found
    and fixed a real test-isolation bug (shared key racing under parallel
    `cargo test`, then a counter-only "fix" that still collided across
    separate `cargo test` invocations until the process id was added too).
  - **7b — `PostgresStore`**: `Store` backend on real Postgres, upsert
    checkpoints. Same `async fn` conversion for `Store`. Explicitly flagged
    (not silently skipped) what it didn't fix: `RdpAccountant`'s epsilon
    still reset on restart.
  - **7c — Observability**: every `eprintln!`/`println!` operational log
    (buffer flush, reputation rejection, privacy budget, round loop, node
    retry/backoff, registry/store backend errors) converted to structured
    `tracing` events, verified firing with real fields via `tracing-test`.
    ADR 0007's config-resolution log lines deliberately left alone (exact
    spec-mandated format, tested byte-for-byte).
  - **7d — `RdpAccountant` persistence**: closed the gap 7b flagged.
    `PrivacyRoundLog` trait (`PostgresStore` implements it), `AppState`
    replays persisted rounds into a fresh accountant before serving its
    first round. Proven with a real "simulated restart" test — two
    independent `AppState`s against the same Postgres table, the second
    one's epsilon matching the first's recorded history, not zero.
  - **7e — mTLS for push mode**: `conflux-net::tls` (`ServerTlsConfig`/
    `ClientTlsConfig` builders), `PullTransport`/`PushTransport::
    connect_with_tls`. Proven with 3 real handshake tests using `rcgen`-
    generated certs: a trusted-CA client completes a real RPC; an
    untrusted-CA client is genuinely rejected (proven via RPC failure,
    since the handshake completes lazily so `connect()` alone isn't
    sufficient evidence); a plaintext client is rejected by an
    mTLS-required server.
  - **7f — `S3Store`**: third `Store` backend, on real MinIO
    (`aws-sdk-s3` against a custom endpoint — genuinely S3-compatible, not
    MinIO-specific). PutObject's natural overwrite semantics mean no
    upsert logic needed, unlike `PostgresStore`.
  - **7g — Load testing**: 30 concurrent `PullTransport` clients × 3
    rounds against one real running server, run 5 times total. All 90
    client-rounds across all 5 runs succeeded (28–46ms/round); the known
    `RoundBuffer` race was not observed at this scale — reported as
    evidence, not proof the race is closed.
  - **A real cross-crate dependency conflict found and fixed**: adding
    `aws-sdk-s3` (7f, pulls in rustls/`aws-lc-rs`) alongside
    `conflux-net`'s existing `tls-ring` choice (7e) meant
    `cargo test --workspace`'s feature unification linked *both* crypto
    providers into `conflux-net`'s own test binaries, and rustls panicked
    at runtime unable to pick one — even though `cargo test -p conflux-net`
    alone never surfaced it (that build plan excludes `conflux-store`
    entirely). Fixed by switching `conflux-net` to `tls-aws-lc`, matching
    the AWS SDK's provider so only one is ever linked.
  - 93 tests passing workspace-wide (was 69 at the end of Phase 6);
    `cargo fmt --check` and `cargo clippy --workspace --all-targets` both
    clean; stable across repeated full-workspace runs (checked 3+ times
    after every sub-phase, not just once).
  - Three Docker containers left running for continued work:
    `conflux-dev-redis` (16379), `conflux-dev-postgres` (15432),
    `conflux-dev-minio` (19000 API / 19001 console). Clean up with
    `docker rm -f conflux-dev-redis conflux-dev-postgres conflux-dev-minio`
    whenever no longer wanted.

- **Documentation**: `docs/USAGE.md` (build/run/quick-start, config env-var
  table, durable-backend setup, mTLS, load testing), `docs/ARCHITECTURE.md`
  (crate graph, round pipeline, two-axis config, family pattern, phase
  history with real bugs found, mermaid diagrams throughout), and
  `docs/FLOWER_COMPARISON.md` (component mapping against a real Flower/
  Wellmatix deployment, convergences, and 5 numbered gaps — 2 and 3 of
  which motivated Phase 8's node-auth and fail-fast-backend-selection
  work below).
- **Phase 8a — hybrid backend selection**: `conflux-registry::AnyRegistry`
  and `conflux-store::AnyStore` (enum-dispatch, not `dyn`, since `Registry`/
  `Store`'s native `async fn` methods aren't dyn-compatible without extra
  boxing). `AppState::connect(config, mode, initial_weights, backends)` —
  the new general async constructor — resolves each of registry/store/
  accounting-persistence independently via `BackendSelection`, and calls
  `validate_production_backends` first: `mode = production` refuses to
  start on any backend still resolving to its in-memory/disabled default,
  naming exactly which env var is missing (mirrors `allow_stub_client`'s
  existing fail-fast shape; directly motivated by the Flower cross-check's
  Problem 3). `AppState::new`/`new_with_persistent_accounting[_table]` kept
  their exact signatures and behavior — zero caller-visible change for
  every pre-Phase-8 test. `main.rs` now reads
  `CONFLUX_REGISTRY_BACKEND`/`CONFLUX_REDIS_URL`,
  `CONFLUX_STORE_BACKEND`/`CONFLUX_POSTGRES_URL`/`CONFLUX_S3_*`,
  `CONFLUX_ACCOUNTING_PERSISTENCE` (reuses `CONFLUX_POSTGRES_URL`) and
  calls `AppState::connect`, closing the "backends exist but nothing wires
  them into the binary" gap Phase 7's status flagged. Smoke-tested
  directly (research-mode in-memory default, production fail-fast, and a
  real production run against Redis+Postgres) since `main.rs` itself isn't
  covered by `cargo test`. 105 tests passing workspace-wide (was 93 at the
  end of Phase 7), stable across repeated runs; `cargo fmt --check` and
  `cargo clippy --workspace --all-targets` both clean.

- **Phase 8b — node auth core**: `require_node_auth: bool` added to
  `conflux-config`, identical shape to `allow_stub_client` (research
  default `false`, production default `true`, full precedence-chain and
  provenance-logging support). `conflux-registry` gained the allow-list
  data model: `NodeIdentity` (`CertFingerprint(String)` from an mTLS peer
  cert's SHA-256 fingerprint, or `SharedToken(String)` for deployments
  without mTLS), `NodeAllowlist` trait (`allow`/`revoke`/`check`/`list`),
  `InMemoryNodeAllowlist` (research), `RedisNodeAllowlist` (production,
  one Redis hash keyed by client id), and `AnyNodeAllowlist` (same
  enum-dispatch pattern as Phase 8a's `AnyRegistry`/`AnyStore`). `check`
  returns `Result<bool, _>`, not a bare `bool`, so a backend outage is
  distinguishable from a genuine denial — matters once Phase 8c has to
  decide how to react to each. 122 tests passing workspace-wide (was 105
  at the end of Phase 8a), stable across repeated runs; `cargo fmt --check`
  and `cargo clippy --workspace --all-targets` both clean. This is the
  data-model half of gap 2/3 from `docs/FLOWER_COMPARISON.md` — nothing
  enforces the allow-list yet (Phase 8c).

- **Phase 8c — node auth enforcement**: `conflux-net` gained
  `peer_cert_fingerprint(&Request<T>) -> Option<String>` (needs tonic's
  `tls-connect-info` feature), extracting and SHA-256-hashing the peer's
  leaf certificate from an mTLS connection's `TlsConnectInfo` extension.
  `RoundDispatcher::register` grew a `peer_cert_fingerprint: Option<&str>`
  parameter (every implementation workspace-wide updated).
  `AppState::node_allowlist: Arc<AnyNodeAllowlist>` is now always
  constructed (`InMemoryNodeAllowlist` for `new`/
  `new_with_persistent_accounting[_table]`; `AppState::connect` derives
  the backend from `backends.registry`, so `CONFLUX_REGISTRY_BACKEND=redis`
  gets `RedisNodeAllowlist` too, one fewer env var than a fully
  independent axis). `conflux-server::dispatcher.rs`'s `register()`
  checks `config.require_node_auth.value` first — builds the presented
  `NodeIdentity` (cert fingerprint if present, else `SharedToken
  (auth_token)`), calls `node_allowlist.check`, and rejects with
  `DispatchError::NotAllowed` → `Status::permission_denied` *before*
  `conflux-registry` is touched at all; behavior is byte-for-byte
  unchanged when the flag is off. New HTTP admin endpoints: `POST`/`GET
  /admin/allowlist`, `DELETE /admin/allowlist/{client_id}`. `main.rs`
  needed no new wiring — `require_node_auth` was already covered by the
  existing config-log loop, and the allow-list backend follows
  `AppState::connect`'s own registry-backend choice.

  Real end-to-end tests (`crates/conflux-server/tests/node_auth.rs`, 7
  tests) cover: an allowed `SharedToken` client registers; a wrong token
  is rejected; a never-allowed client is rejected; a revoked client is
  rejected; `require_node_auth = false` keeps registration working with
  an empty allow-list; and — the specific case
  `docs/FLOWER_COMPARISON.md` flagged as missing — an mTLS client whose
  cert is signed by the trusted CA but was never `allow`-ed is rejected
  even though the TLS handshake itself succeeds, while one that *is*
  allow-listed registers. Plus a real fingerprint-extraction test in
  `conflux-net/tests/mtls.rs` (compares the extracted fingerprint against
  an independent SHA-256 of the client cert's DER bytes) and an HTTP
  admin round-trip test (add/list/revoke/confirm).

  131 tests passing workspace-wide (was 122 at the end of Phase 8b),
  stable across repeated runs; `cargo fmt --check` and
  `cargo clippy --workspace --all-targets` both clean. Smoke-tested the
  binary directly against real Redis + Postgres with
  `CONFLUX_MODE=production` — `require_node_auth` resolves and logs
  `true`, `RedisNodeAllowlist` connects, no panics.

  **Gaps 2 and 3 from `docs/FLOWER_COMPARISON.md` are now closed** — that
  document itself has been updated to say so, not just this file.

- **Phase 9a — `auth` enforcement**: `conflux-server::auth_enforcement::
  resolve_server_tls(mode, auth, material) -> Result<Option<ServerTlsConfig>, _>`
  — a pure decision function (5 unit tests): `auth = jwt` always
  plaintext; `auth = mtls` with real material binds TLS in either mode;
  `auth = mtls` with no material falls back to plaintext (logged warning)
  in research, fails fast in production. `main.rs` reads
  `CONFLUX_TLS_CERT_PATH`/`CONFLUX_TLS_KEY_PATH`/
  `CONFLUX_TLS_CLIENT_CA_PATH` and conditionally applies `.tls_config(...)`
  to the gRPC builder. Real tests (`tests/auth_enforcement.rs`) prove real
  `rcgen` material produces a server a trusted-CA client can use and a
  plaintext client can't. Smoke-tested all three states directly on the
  binary. Closes gap 4.
- **Phase 9b — production stub-client guard**: closes gap 5, in
  `conflux-node` rather than `conflux-server` (the spec's original wording
  named the wrong process — `conflux-server` never talks to Python at
  all; only `conflux-node` has the local loopback listener, ADR 0004).
  `conflux-node::startup_guard::validate_client_app_startup(mode,
  allow_stub_client, kind)` — `RuntimeMode`/`ClientAppKind` defined
  locally rather than adding a `conflux-config` dependency (preserves
  Phase 6's deliberate scope decision). Since ADR 0005 defers the real
  Python SDK entirely, there's no protocol-level way to detect a real vs.
  stub `ClientApp`; the guard is an explicit operator assertion
  (`CONFLUX_CLIENT_APP_KIND=stub|real`), refusing to start in production
  with the (default, and currently only real) stub kind unless overridden.
  Smoke-tested: production+stub fails before any upstream connection
  attempt; `CONFLUX_CLIENT_APP_KIND=real` passes the guard cleanly; the
  existing Phase 6 default research path (`docs/USAGE.md`'s quick-start)
  is unchanged.
  - 142 tests passing workspace-wide (was 131 at the end of Phase 8),
    stable across repeated runs; `cargo fmt --check` and
    `cargo clippy --workspace --all-targets` both clean.
  - **All four gaps (2–5) from `docs/FLOWER_COMPARISON.md` are now
    closed** — only gap 1 (server-side process isolation) remains, and
    that one is deliberately unaddressed under Conflux's current threat
    model (see that document).

- **Phase 10a — closed the `RoundBuffer` lost-update race**: `RoundBuffer`'s
  `deltas: Mutex<Vec<ClientDelta>>` became `state: Mutex<BufferState>`
  (`Open(Vec<ClientDelta>) | Closed`) — closing is now atomic with taking
  the flush snapshot (a separate `AtomicBool` flag, the brief's original
  suggestion, still leaves a TOCTOU; putting the state inside the same
  mutex closes it by construction instead). New `BufferError::Closed` →
  `conflux-net::DispatchError::RoundClosed` (`Status::
  failed_precondition`) — a submission racing an already-flushed round
  (the real window a retried `run_round` leaves open on
  `AggregatorError::EmptyBatch`) is now explicitly rejected instead of
  silently accepted into a buffer nobody reads again. Reproduced directly
  with a 200-iteration multi-threaded race test and an end-to-end
  `conflux-server` test driving the actual retry precondition. 145 tests
  passing, stable across 3 runs.
- **Phase 10b — wired the strategy registry into real selection**: closes
  ADR 0002's deferred half — `conflux-core`/`conflux-selector` each
  register their one family member
  (`inventory::submit!{"fedavg"}`/`{"uniform_random"}`) and expose a
  `build_*(name) -> Result<Box<dyn _>, _>` factory; `AppState::assemble`
  now actually reads `config.aggregator.value`/`config.selector.value`
  instead of hardcoding `FedAvg::default()`/`UniformRandomSelector`.
  `AppState::new`'s signature stayed byte-for-byte unchanged (every
  pre-Phase-10 test passed with zero modification) — `assemble` stays
  infallible and `.expect()`s on an unknown name, the same
  "startup-invariant, not a runtime `Result`" treatment `main.rs` already
  gives config resolution itself. Real test: an explicit
  `aggregator="fedavg"`/`selector="uniform_random"` override resolves
  through the registry and completes a live round end-to-end; a
  `catch_unwind` test confirms an unregistered name panics loudly rather
  than silently falling back. 155 tests passing, stable across 3 runs.
  - `cargo fmt --check` and `cargo clippy --workspace --all-targets` both
    clean throughout Phase 10.

- **Phase 11a — redesigned aggregation architecture + the `robust`
  family**: shipped Krum, Multi-Krum, Trimmed Mean, Median (spec §5/§10),
  citing Blanchard, El Mhamdi, Guerraoui & Stainer (2017, NeurIPS) for
  Krum/Multi-Krum and Yin, Chen, Ramchandran & Bartlett (2018, ICML) for
  Trimmed Mean/Median. Per explicit direction, first redesigned
  `conflux-core`'s aggregation shape rather than bolting four one-off
  implementations onto Phase 4b's scaffold: the old `RobustSelection`
  trait fit Krum/Multi-Krum (pick a subset of whole updates) but
  misrepresented Trimmed Mean/Median (inherently coordinate-wise, no
  "selected whole update" per client). Replaced with two composable
  pieces — `UpdateFilter` + `FilteredAggregator<F: UpdateFilter, C:
  Aggregator>` (any existing `Aggregator`, including `FedAvg`, can be the
  combiner) for selection-based members, and
  `CoordinateWiseRobustStatistic` + `CoordinateWiseAggregator<S>` for
  coordinate-wise ones. Renamed `RobustSelection` → `UpdateFilter` to
  stop it colliding in spirit with `conflux-selector::ClientSelector`
  (client sampling, a different pipeline stage entirely). The
  composability claim is proven directly, not just asserted:
  `FilteredAggregator` composed with a non-`FedAvg` combiner
  (`MultiKrumFilter` + `CoordinateWiseAggregator<MedianStatistic>`, a
  combination nothing ships as a named strategy) has its own passing
  test — the concrete shape a future Bulyan-style method (El Mhamdi,
  Guerraoui & Rouault, 2018) would need, with zero new plumbing. New
  `robust_byzantine_fraction` config parameter (builtin fallback `0.2`),
  small-batch clamping tested for `n=1`/`n=2` on every method, and real
  "poison tests" (honest cluster vs. large-magnitude/sign-flipped
  attackers) proving the actual resistance property, not just arithmetic.
  175 tests after this sub-phase.
- **Phase 11b — `privacy_mechanism` registry wiring**: closed the gap
  Phase 10b deliberately deferred — the third and last spec §5 family
  (`dp` privacy) is now registry-wired the same way `aggregator`/
  `selector` were. New `PrivacyMechanism` trait;
  `GaussianClippingPrivacy`'s `add_noise`/`transform` changed from `rng:
  &mut impl rand::Rng` to `rng: &mut dyn rand::Rng` (required for object
  safety) — confirmed a true no-op at every call site via automatic
  unsized coercion (all 7 pre-existing tests passed unmodified).
  `AppState.privacy` is now `Box<dyn PrivacyMechanism>`. 181 tests after
  this sub-phase.
- **Phase 11c — poison mode for the Python stub client**: `stub_client.py
  --poison`/`--poison-magnitude` (default off, zero behavior change for
  every existing invocation), so the `robust` family's poison-resistance
  can be exercised cross-language over the real network hop, not just in
  Rust unit tests. **Manually verified live**: a real 3-process run
  (2 honest `stub_client.py` + 1 `--poison --poison-magnitude 1000.0`,
  server configured with `aggregator = "krum"`) — the first attempt
  raced Phase 10a's `RoundClosed` fix live (an unplanned, genuine
  confirmation it works outside its own test suite); a retry landed all
  three in one round (`quorum=3`, `num_passed=3`), and the resulting
  checkpoint read back as exactly `(1.0, 1.0)` — the honest clients'
  value, with the attacker's `1000.0` submission fully excluded. No Rust
  changes in this sub-phase.
  - `cargo fmt --check` and `cargo clippy --workspace --all-targets` both
    clean throughout Phase 11.

- **Phase 12 — `conflux-attacks`, a known-attack simulation crate**: a
  13th workspace crate (ADR 0010), deliberately outside
  `conflux-server`'s dependency graph — `conflux-proto` normal
  dependency, `conflux-core` as a **dev-dependency only** (`cargo tree -p
  conflux-server` confirmed clean at every depth). Four cited attacks:
  `GaussianAttack` (Blanchard et al. 2017), `SignFlippingAttack` (Li,
  Xu, Chen & Charles 2019), `AlieAttack` — "A Little Is Enough" (Baruch,
  Baruch & Goldberg 2019, the attack specifically designed to evade
  Krum/Trimmed-Mean-style defenses), and `ScalingAttack` (Bagdasaryan et
  al. 2020's model-replacement boosting mechanism, adapted to one
  round's delta rather than full cross-round replacement — a documented
  scope-narrowing). `AlieAttack` needed the inverse standard normal CDF;
  implemented directly (Acklam's public-domain rational approximation,
  ~1.15e-9 accuracy) rather than adding a statistics dependency, tested
  against known Φ⁻¹ table values.
  `crates/conflux-attacks/tests/attack_vs_defense.rs` runs every attack
  against every shipped `Aggregator` (`fedavg` plus the four Phase 11a
  `robust` members) — the actual `aggregate()` call, not mocked.
  **Honest empirical finding, not assumed**: at 8 honest clients (std
  dev 0.3/coordinate) and up to 33% attackers, all four defended
  aggregators held against every attack including ALIE in this parameter
  regime — reported via the test's own output rather than a claim the
  literature's documented failure modes were reproduced (a single-round,
  low-dimensional harness likely isn't the right lens for that; noted as
  a real limitation, not hidden). New `docs/EXTENDING.md` guide: how to
  add a new aggregator (both `robust`-family shapes), selector, privacy
  mechanism, or attack, plus a full audit-and-fix pass across
  `docs/USAGE.md` (the env-var table had been stale since Phase 8a, not
  just missing Phase 11 — rewritten with every `conflux-server`/
  `conflux-node` env var that actually exists today), `docs/ARCHITECTURE.md`
  (family-pattern section, phase-history timeline, crate graph),
  `docs/spec/conflux-spec-v1.md` (§5's stale `RobustSelection` sketch,
  §9's missing `robust_byzantine_fraction`/`require_node_auth` rows,
  §10's phase list — including resolving a "Phase 8" naming collision
  between the spec's old generic placeholder and the real, later,
  unrelated Phase 8 that shipped), `docs/FLOWER_COMPARISON.md`, ADRs
  0002/0008 (already had accurate "Update" sections from Phase 11a/11b —
  confirmed, not re-edited), and one source doc-comment in
  `conflux-core::Aggregator`.
  - 200 tests passing workspace-wide (was 181 at the end of Phase 11),
    stable; `cargo fmt --check` and
    `cargo clippy --workspace --all-targets` both clean.

- **E2E test harnesses built and verified live (2026-08-22)**:
  `docs/E2E_TESTING.md`'s planned Option A (NumPy logistic regression)
  and Option B (real PyTorch MLP on real MNIST) both built under
  `python/conflux_client/examples/` and actually run — not just
  written — against real `conflux-server`/`conflux-node` binaries.
  `main.rs` gained `overrides_from_env()` (`CONFLUX_AGGREGATOR`/
  `CONFLUX_SELECTOR`/`CONFLUX_PRIVACY_MECHANISM`/
  `CONFLUX_ROBUST_BYZANTINE_FRACTION`/`CONFLUX_QUORUM`/
  `CONFLUX_ROUND_TIMEOUT_SECS`/`CONFLUX_CLIP_NORM`/
  `CONFLUX_NOISE_MULTIPLIER`/`CONFLUX_MIN_REPUTATION_SCORE`) and
  `CONFLUX_INITIAL_WEIGHTS_DIM`, closing the gap Phase 11c's manual
  verification had flagged and making the E2E harness runnable without
  any throwaway example binary.

  **Verified results**: both options converge within a couple points of
  a centralized baseline (Option A: 0.735–0.7425 vs. 0.7375; Option B:
  0.905 vs. 0.889 after the fix below), and `krum` holds against a
  persistent Byzantine client on both — 0.72–0.7375 (Option A) and 0.884
  (Option B) — when isolated from a separately-discovered issue (next).

  **Two real findings, not caught by any earlier test**:
  1. `conflux-reputation`'s cosine-similarity filter runs *before*
     aggregation and has its own blind spot — a single large-magnitude
     attacker in round 1 (shared zero-init checkpoint) can skew the
     filter's reference mean enough to reject every honest update,
     starving the aggregator of the honest batch it needs to defend at
     all. Confirmed: with reputation at its default, `krum` and `fedavg`
     both collapsed identically (0.3975) against the same attacker —
     the aggregator choice made zero difference, because it never saw
     the honest clients either way. **Open, not fixed** — see "Next"
     below.
  2. Conflux's placeholder initial checkpoint (`vec![0.0f32; N]`) is a
     textbook symmetry-breaking failure for a ReLU network — every
     hidden unit starts identical with an identical zero gradient and
     can never differentiate. Option B's first real run showed accuracy
     pinned at random-guessing level (~0.11) for its entire run. Fixed
     in the harness (client-side placeholder detection + substitution,
     `is_placeholder_init` in `e2e_pytorch_mnist/model.py`), which is
     also the architecturally correct place for the fix per ADR 0004 —
     `conflux-server` genuinely cannot know the right init for an
     arbitrary model.

  Full write-up, both findings, in `docs/E2E_TESTING.md`. Each example
  has its own `README.md` (prerequisites, usage, troubleshooting)
  written for someone running this framework for the first time.
  `cargo fmt --check`/`cargo clippy --workspace --all-targets` clean,
  200 Rust tests still passing (this work was Python-harness- and
  `main.rs`-env-var-focused, not a new Rust test surface).

- **`docs/WEB_APP_INTEGRATION.md` + `CONFLUX_GRPC_ADDR`/
  `CONFLUX_HTTP_ADDR` (2026-08-22)**: wrote up how a web app in any stack
  (FastAPI, Django, Node, Axum) integrates with Conflux — the real
  interface is `conflux-server`'s existing HTTP admin router
  (`/health`, `/round/status`, `/admin/allowlist*`), called the same way
  regardless of caller language; the gRPC `FlTransport` service is for FL
  clients only, not the platform backend. Writing this up surfaced a real
  gap it then closed: both listeners were hardcoded to `127.0.0.1` in
  `main.rs`, unreachable from a backend running in a different container.
  `main.rs` now reads `CONFLUX_GRPC_ADDR`/`CONFLUX_HTTP_ADDR` (default
  unchanged — still `127.0.0.1:50051`/`:8080` when unset, since the admin
  API still has no auth of its own and shouldn't bind wider by default).
  Verified directly: overridden port responds on `/health`, default port
  stays correctly unbound, and an invalid address panics with a clear
  message before the server starts (same fail-fast pattern as every other
  `CONFLUX_*` var). `cargo fmt --check`/`clippy --workspace --all-targets`
  clean, 200 tests still passing.

- **Project renamed Confluo → Conflux (2026-08-22)**: every `confluo-*`
  crate directory (all 13, plus their package names and inter-crate path
  dependencies), the `python/confluo_client` → `conflux_client` package,
  every `CONFLUO_*` env var → `CONFLUX_*`, the `confluo.v1` proto package
  → `conflux.v1` (regenerated Python stubs from it), `docs/spec/
  confluo-spec-v1.md`/`confluo-development-plan.md` →
  `conflux-spec-v1.md`/`conflux-development-plan.md`, and every doc
  cross-reference across the ADRs, phase briefs, and top-level docs — all
  mechanically replaced, case-sensitive (`Confluo`→`Conflux`,
  `confluo`→`conflux`, `CONFLUO`→`CONFLUX`), verified with a final
  repo-wide sweep showing zero remaining `confluo` references anywhere
  outside `target/`/`.git`/`.venv`. ADR 0009 (project naming) got an
  "Update" section documenting the rename rather than a second ADR, per
  its own closing line ("revisited rather than silently renaming"). Two
  local dev containers (`confluo-dev-postgres`, `confluo-dev-minio`) had
  hardcoded credentials/db names baked in from creation and needed
  recreating under the new names/credentials (`conflux-dev-postgres`,
  etc., per `docs/USAGE.md`'s updated `docker run` commands) before their
  dependent integration tests passed again — a one-time local-environment
  sync cost of the rename, not a code issue. Full verification: `cargo
  build --workspace` clean under the new crate names, `cargo fmt --check`/
  `clippy --workspace --all-targets` clean, `cargo test --workspace`
  stable at 200 tests across two runs (including the two
  Postgres-dependent integration tests, once the containers were
  recreated).

- **Dirichlet non-IID E2E runs + reputation-fix phase brief (2026-08-23)**:
  both `run_demo.sh` scripts gained a `--dirichlet [--dirichlet-alpha N]`
  flag (previously `--split dirichlet` existed in `partition_data.py` but
  no harness script exposed it). Run live on both options: at
  `alpha = 0.5`, both converge close to their centralized baselines
  despite real per-client heterogeneity (Option A: 0.7275 vs. 0.7375;
  Option B, real MNIST: 0.891 vs. 0.889). At an aggressive `alpha = 0.1`,
  Option A produced a **zero-sample client shard**, whose resulting `NaN`
  gradient poisoned `conflux-reputation`'s shared batch-mean reference —
  `NaN` propagates through arithmetic and `NaN >= threshold` is always
  `false`, so **all five clients**, not just the broken one, were
  rejected every round; accuracy froze at 0.4975 for the whole run. A
  third real finding (`docs/E2E_TESTING.md`'s "Real findings" #3),
  distinct from the pipeline-order finding above — no attacker required,
  and a coordinate-wise-median fix for the outlier finding doesn't fix
  this one (`NaN` poisons a median exactly as it poisons a mean).

  This fed directly into `docs/phases/phase-13-reputation-reference-fix.md`
  (draft, not started) — the scoping brief for the reputation fix, which
  also **corrects** `docs/AGGREGATION_LANDSCAPE.md`'s earlier
  recommendation: that doc argued for an FLTrust-style independent
  trusted reference as the strongest fix, but a closer read of the
  codebase during scoping found this requires the *server* to train its
  own reference update on real data — directly conflicting with ADR
  0004's boundary that `conflux-server` never trains anything. The
  trusted-reference approach is now recorded as a separate, deferred,
  ADR-0004-revisiting question, not this phase's scope. Phase 13's actual
  recommended scope: (1) reject non-finite (`NaN`/`Inf`) submissions in
  `round.rs::decode_flushed_deltas` before any reference computation
  touches them — fixes finding 3 directly; (2) replace `round.rs:72`'s
  `mean_vector` with a coordinate-wise median, reusing
  `conflux-core::MedianStatistic` (a new `conflux-reputation` →
  `conflux-core` dependency edge, no cycle) rather than reimplementing
  robust-statistic logic twice — fixes finding 1's specific reproduced
  attack shape, explicitly *not* claimed as a complete Byzantine-fraction
  defense. Full design, deliverables, test plan, and open questions in
  that phase brief.

- **Phase 13 revised — reputation filtering becomes opt-in, not more
  robust (2026-08-23)**: project-owner guidance corrected the premise
  the entry above and `docs/AGGREGATION_LANDSCAPE.md` were written
  under. Conflux's purpose is a faithful, extensible catalog of every
  published aggregation method — each behaving exactly as its own paper
  defines it, never modified by the framework — for researchers to use
  as literal comparison baselines; architecture priority is keeping the
  family pattern (ADR 0002) simple so more methods stay cheap to add,
  not minimizing attack surface. Under that lens, `conflux-reputation`'s
  `CosineScorer` applied unconditionally in front of *every* aggregator
  was itself the bug — no cited paper (Krum, Trimmed Mean, Median, ...)
  asks for an extra uncited filter ahead of it. **Phase 13's scope is
  now simpler**: reputation filtering becomes opt-in (`conflux-config`
  gains `reputation_filter_enabled: bool`, default `false`) instead of
  mandatory — every aggregator's default behavior matches its paper with
  zero interference, which fixes finding 1 outright. The
  coordinate-wise-median plan from the entry above is dropped (it was
  solving robustness for a mandatory gate that no longer exists); the
  non-finite (`NaN`/`Inf`) rejection fix for finding 3 still stands,
  since that's a plain correctness bug independent of the default.
  Methods with their own published trust mechanism (FLTrust, Zeno) would
  be built as self-contained aggregators via the normal family-pattern
  process, not as `conflux-reputation` extensions, if/when prioritized.
  Full account in the phase brief's "Revision history" and
  `docs/AGGREGATION_LANDSCAPE.md`'s second "Update" section.

- **Six new aggregators + a research proposal (2026-08-23)**: catalog
  grew from 4 to 10 named aggregators. `crates/conflux-core/src/robust.rs`
  gained **FABA** (Xia, Zhang, Yang, Shao & Yin, 2019 — `UpdateFilter`),
  **Bulyan** (El Mhamdi, Guerraoui & Rouault, 2018 —
  `FilteredAggregator<BulyanFilter, TrimmedMean>`, a documented
  combiner-trim simplification noted in its own doc comment), **Median-of-
  Means** (Chen, Su & Xu, 2017 — `CoordinateWiseRobustStatistic`, groups
  by array position, consistent across coordinates for free), and
  **Divide-and-Conquer** (Shejwalkar & Houmansadr, 2021 — new
  `top_singular_vector` power-iteration helper, `UpdateFilter`, the
  paper's own `b = full dim, niters = 1` special case, documented as
  such). A new **`RobustVectorStatistic`** trait + `VectorRobustAggregator<S>`
  (a third family shape, whole-vector rather than per-coordinate or
  selection-based) ships **Geometric Median / RFA** (Pillutla, Kakade &
  Harchaoui, 2019/2022 — Weiszfeld's algorithm, weighted by `num_samples`
  per the paper's own FL-specific formulation, unlike this crate's
  deliberately-unweighted Trimmed Mean/Median). A new
  `crates/conflux-core/src/temporal.rs` module ships **FoolsGold** (Fung,
  Yoon & Beznosov, 2018/2020) — the first aggregator needing cross-round
  state (`Mutex<HashMap<client_id, accumulated_history>>`, since
  `Aggregator::aggregate` takes `&self`); its Sybil-detection test is
  hand-computed and matches exactly (two colluding clients submitting
  identical updates every round collapse to the honest-only average,
  `[0.667, 0.0]`, not the unweighted all-five average `[2.4, 1.4]`).
  Every new method: real numeric/poison tests (not just "doesn't crash"),
  registered in the `inventory::submit!`/`build_aggregator` registry,
  and a `conflux-server` end-to-end integration test. `cargo fmt --check`/
  `clippy --workspace --all-targets` clean, 200+ tests stable across two
  runs (the exact count grew with the new tests — see each crate's own
  test output for the current total, not restated here to avoid this
  entry going stale the next time a test is added).

  Also: `docs/research/temporal-consistency-aggregation.md` — a
  publication-track research proposal identifying a gap **all ten**
  methods share (every one judges a round's batch in isolation, so none
  can distinguish a colluding Sybil cluster from a legitimate majority,
  and none can distinguish "malicious" from "legitimately, consistently
  non-IID"). Proposes **Deviation Stability Scoring (DSS)**, a
  cross-round wrapper hypothesis, with a full experimental plan (attacks,
  metrics, datasets, statistical rigor) — explicitly a proposal, not a
  validated result or a framework change, consistent with this project's
  governing principle (faithful catalog, not a defense platform — see
  the reputation-filtering revision above). FoolsGold was built as part
  of this — a real, citable method that already partly fills the gap,
  independent of whether DSS itself ever gets built.

- **Phase 13 shipped — reputation filtering is opt-in (2026-08-23)**:
  `conflux-config` gained `reputation_filter_enabled: bool` (`Overrides`-
  only, builtin fallback `false`), wired through `resolve()`/
  `to_log_lines()` like every other parameter. `round.rs` skips the
  `mean_vector`/`filter_by_threshold` stage entirely when the flag is
  off — every aggregator's default behavior now matches its cited paper
  with zero framework-imposed interference, fixing finding 1 outright
  (proven directly: `krum` defends against a large-magnitude outlier
  with the new default, no `--no-reputation` workaround needed).
  `decode_flushed_deltas` also now excludes any non-finite (`NaN`/`Inf`)
  submission — logged, not a whole-round failure — regardless of the
  flag, fixing finding 3. `main.rs` gained
  `CONFLUX_REPUTATION_FILTER_ENABLED`. Three new integration tests in
  `crates/conflux-server/tests/reputation_opt_in.rs`, each hand-verified
  against the actual cosine-similarity mechanism (not just "doesn't
  crash"). `cargo fmt --check`/`clippy --workspace --all-targets` clean,
  full suite stable across two runs.

- **FoolsGold corrected against the authors' reference implementation
  (2026-08-23)**: the previous entry's FoolsGold was implemented from
  the paper's prose description; the user supplied the actual reference
  code (<https://github.com/DistributedML/FoolsGold>,
  `deep-fg/fg/foolsgold.py`/`trainer.py`), which turned up two real
  discrepancies — the "pardoning" step loops over *every* client pair
  comparing each pair's own max-similarity (not just each row's argmax,
  and the earlier version also had the comparison direction backwards),
  and the combine step divides by client count `n` (matching the
  reference's own `aggregate_gradients`), not by the sum of trust
  weights or by `num_samples` like every other aggregator in this crate
  — a deliberate, documented exception, since this is the one method
  where matching the original paper's exact experimental setup matters
  more than this codebase's usual weighting convention. Rewrote
  `foolsgold_weights` as a direct line-by-line translation of the
  reference; tests re-verified by hand against the corrected math
  (worked example: 3 mutually-orthogonal honest histories + 2 identical
  colluders converge to weights `[1.0, 1.0, 1.0, 0.0, 0.0]` exactly).
  `cargo fmt --check`/`clippy` clean, tests stable.

- **`PersistentSybilAttack` + experiment infrastructure + real Section 2
  results (2026-08-23)**: `conflux-attacks` gained `PersistentSybilAttack`
  — every other attack in the crate crafts its output as a function of
  *that round's* honest batch, so its raw output drifts round to round
  even with fixed parameters; this one submits the exact same update
  every round regardless, the scenario `FoolsGoldAggregator` and any
  future temporal defense need to be tested against (`docs/research/
  temporal-consistency-aggregation.md`, Section 2.2). New application
  test: `foolsgold_defends_against_persistent_sybil_collusion_across_rounds`
  (beats undefended FedAvg every round over a 5-round simulation).

  Ran as `crates/conflux-attacks/examples/run_experiment.rs` — deliberately
  **not** a new workspace crate (`conflux-experiments`, this entry's
  first draft) after re-examining that choice: it's a research/dev tool,
  not a product component, and `conflux-attacks` already carries
  `conflux-core` as a dev-dependency for its own `tests/
  attack_vs_defense.rs` (ADR 0010) — examples can use dev-dependencies
  too, so this needed no new crate, no new workspace member, and no new
  ADR to justify a 14th crate the spec's stated 13-crate layout doesn't
  mention. Prints one JSON line per (aggregator × attack × collusion
  size × round) invocation to stdout. Three shell scripts in
  `docs/research/scripts/` (`experiment_2_1_collusion_scaling.sh`,
  `experiment_2_2_persistent_collusion.sh`, `summarize.py` — JSONL → CSV,
  no dependencies beyond the standard library) sweep the actual
  parameter grids from the research proposal's Section 2 and write
  results to `docs/research/results/`.

  **Both experiments actually run, not just built** — real results in
  `docs/research/results/*.jsonl`/`*.csv`. Two genuine findings:
  1. Experiment 2.1 (187 rows): `ScalingAttack` at higher collusion
     counts (`scale_factor=5`) defeats every aggregator in the catalog,
     including the `robust`-family ones — consistent with why boosted/
     scaled attacks are the literature's own backdoor-attack mechanism
     (Bagdasaryan et al., 2020, cited on `ScalingAttack` itself already).
  2. Experiment 2.2 (440 rows, 20 rounds × 11 aggregators × 2 attacks —
     `PersistentSybilAttack` and, since 2026-08-23, `AdaptiveEvasionAttack`
     too, see below): `foolsgold` performs **worse** than every
     single-round `robust` method against both (mean distance ~1.2–1.55
     vs. ~0.18–0.32), because its reference-matched combine step divides
     by total client count `n`, not the honest survivor count — so even
     perfect Sybil detection still dilutes the result by the excluded
     clients' share. A real, faithful property of the original paper's
     algorithm (not a bug in this implementation, and not "fixed" here,
     per this project's own faithful-catalog principle), and a concrete,
     measured reason `docs/research/
     temporal-consistency-aggregation.md`'s Deviation Stability Scoring
     hypothesis should renormalize by the trusted survivor weight if it
     gets built, rather than inheriting this specific property.

- **`AdaptiveEvasionAttack` (2026-08-23)**: `conflux-attacks`' Section
  2.2 stretch goal, built. The `Attack` trait gained
  `craft_adaptive(&self, honest_updates, num_attackers, feedback:
  Option<&RoundFeedback>)`, with a default implementation that just
  calls `craft` — every existing attack (`Gaussian`/`SignFlipping`/
  `Alie`/`Scaling`/`PersistentSybil`) needed zero code changes. `
  RoundFeedback { previous_submission, previous_aggregate }` is built
  from plain `Vec<f32>`s every `Aggregator` already produces, so it
  works uniformly across the whole catalog regardless of family shape —
  not just `UpdateFilter` members with a `SelectionResult` to inspect.
  `AdaptiveEvasionAttack` itself: a local hill-climbing heuristic (not a
  reproduction of Fang et al. 2020's optimization search, documented as
  such) — escalate magnitude ×1.2 when last round's submission mostly
  survived into the aggregate, retreat ×0.5 when it got pulled back
  toward honest consensus. Deterministic tests verify exact compounding
  (10.0 → 12.0 → 14.4 across two successful rounds). Wired into
  `run_experiment` and `experiment_2_2_persistent_collusion.sh`
  (now sweeps both `persistent_sybil` and `adaptive_evasion`, 440 rows
  total) — `foolsgold` shows the same dilution weakness against this
  harder, reactive attacker too, consistent with the persistent-sybil
  finding above.

- **Experiment 2.1b — `byzantine_fraction`-matched confirmation
  (2026-08-23, 176 rows)**: re-ran Experiment 2.1's `ScalingAttack` sweep
  with `byzantine_fraction` computed dynamically per point
  (`(num_attackers+1)/total_clients`, capped at 0.49) instead of a fixed
  guess, isolating whether the attack has any advantage beyond parameter
  mismatch. It doesn't — every previously-collapsing survivor-count-
  bounded method (Multi-Krum, FABA, Divide-and-Conquer, Trimmed Mean)
  stays in the 0.05–0.4 distance range once correctly parameterized,
  confirming the original 187-row result was a parameter-mismatch
  artifact, not a fundamental weakness in those methods
  (`experiment_2_1b_matched_byzantine_fraction.sh`).
- **Multi-seed statistical rigor (2026-08-23)**: Experiment 2.1 now
  defaults to 5 seeds (935 rows, was 187) and Experiment 2.2 to 5
  independent 20-round repeats (2,200 rows, was 440), both overridable
  via a second script argument. `summarize.py` gained a `ci95()` helper
  (95% CI via normal approximation, `1.96*stdev/sqrt(n)`, documented as
  a stdlib-only simplification of the more correct t-distribution CI,
  adequate at n≥5) plus `n_seeds`/`ci95_distance_from_true_value`/
  `ci95_asr` summary columns.
- **`AdaptiveEvasionAttack` v2 (2026-08-23)**: the original heuristic
  (comparing `pulled_fraction` against a fixed 0.5 threshold) had a real
  bug, found via honest reporting of its own real-data behavior: it
  couldn't distinguish "a real defense suppressed me" from "I'm 2 of 10
  clients, so any weighted average dilutes me" — it retreated even
  against fully undefended `fedavg`. Fixed by computing the *expected*
  dilution an undefended weighted average would have produced from last
  round's honest batch + submission, and only treating suppression
  beyond that baseline (+ a 0.15 margin) as evidence of a real defense.
  Re-verified against real data: `fedavg`'s distance under the fixed
  attack now climbs monotonically and without bound (mean 161.3,
  last-round 553.0 across 5 repeats — was flat/bounded under v1's bug),
  while every defended method's numbers stay within noise of the
  non-adaptive `PersistentSybilAttack` baseline.
- **Experiment 2.3 — non-IID fairness (2026-08-23, 10,560 rows)**: new
  `run_fairness_experiment` example + `experiment_2_3_noniid_fairness.sh`
  (11 aggregators × 6 minority-shift values × 20 seeds, zero attackers).
  Measurement design: leave-one-out influence
  (`‖A(batch) − A(batch∖{i})‖`), normalized against `fedavg`'s own
  leave-one-out influence on the identical batch/seed (raw influence is
  confounded by a point's own extremity, not just whether it's
  discriminated against — `fedavg` applies no filtering, so it's the
  "no discrimination" reference). Result: a clean structural split, not
  a uniform effect — Krum/Median/Bulyan/Geometric-Median show a strong
  fairness cost (minority influence collapses to 22–39% of baseline at
  high divergence); Multi-Krum/FABA/Divide-and-Conquer show weak-to-no
  cost (flat 1.28–1.44 across the whole range); Trimmed Mean is a
  genuine, not-fully-explained anomaly (influence *increases* with
  divergence). Full writeup: `docs/research/
  temporal-consistency-aggregation.md` §5.4.
- **Deviation Stability Scoring (DSS) — implemented and validated
  (2026-08-23)**: `DssAggregator` (`crates/conflux-core/src/temporal.rs`)
  — a cross-round wrapper around any existing `Aggregator`, per §6.2 of
  the research proposal. Deliberately **not** in `build_aggregator`'s
  string-based catalog (a research hypothesis, never a framework
  default, per this project's faithful-catalog principle) — constructed
  directly (`DssAggregator::new(base)`) or via `run_experiment`'s
  `--aggregator dss_<base>` convenience prefix. Folds in Experiment
  2.2's own dilution finding before any code was written (renormalizes
  by trusted weight sum, not raw client count). 4 new unit tests, 51
  total in `conflux-core`. Validated against real data — Experiment 2.4
  (1,400 rows, `experiment_2_4_dss_validation.sh`), same design as
  Experiment 2.2 — with one confirming and two limiting findings:
  1. Wrapping `fedavg` in DSS converts its catastrophic, unbounded
     `adaptive_evasion` failure (mean 161.3) into a small, bounded one
     (mean 1.18) — real evidence for the core hypothesis, on the one
     attack shape (temporally *unstable* colluders) DSS actually targets.
  2. DSS provides **zero** protection against `persistent_sybil` (stable
     colluders) — predicted by its own unit tests: a stable attacker's
     low deviation variance keeps its stability score high, so the
     stability-AND-collusion gate never fires.
  3. Wrapping an already-robust base method (Krum, Multi-Krum) in DSS
     can make results **worse** than the unwrapped base (`dss_krum` vs.
     `persistent_sybil`: mean 16.99, ~57× worse than plain `krum`'s
     0.297) — DSS's combine step uses the base method's output only as a
     deviation reference, never to gate the final weights, so whenever
     DSS's own gate doesn't fire the combine silently degrades to a
     plain weighted mean of everyone's raw submission, discarding the
     base method's own exclusion. Not predicted in advance — found only
     by measuring. Full writeup, including the practical recommendation
     (DSS-on-`fedavg` only, for now): `docs/research/
     temporal-consistency-aggregation.md` §5.5, §6.4.

- **DSS novelty positioning + 4 new stress-test experiments
  (2026-08-24)**: user asked whether DSS is a novel research
  contribution and requested a literature comparison plus more
  experiments justifying the claim — both done. `docs/research/
  temporal-consistency-aggregation.md` §6.5 positions DSS's actual
  mechanism against the closest real prior art (FoolsGold — history-
  based collusion detection, but on raw gradient vectors, not scalar
  deviation traces; Karimireddy, He & Jaggi 2021's Centered Clipping —
  the paper establishing cross-round information helps Byzantine
  robustness, newly added to References; FLTrust/Zeno — the trusted-
  external-reference family DSS deliberately isn't), concluding DSS is a
  genuine but narrow contribution: a specific synthesis of existing
  ideas (temporal-variance stability + trace-similarity collusion,
  AND-gated) assembled against this project's own Claim 1/Claim 2
  formalization, not a new algorithmic primitive.
  4 new real experiments substantiate that comparison (19,871 total
  rows now, up from 15,271) rather than leaving it as architectural
  argument:
  1. **Mechanism ablation** (§5.6, Experiment 2.5, 600 rows,
     `experiment_2_5_dss_ablation.sh`): stability-only and collusion-only
     variants built by setting `DssAggregator`'s already-`pub`
     thresholds to extreme values (`dssstab_`/`dsscoll_` prefixes in
     `run_experiment.rs`, no code change to `DssAggregator` needed).
     Finding: in this document's synthetic collusion model (identical
     Sybil submissions), the AND-gate is numerically identical to
     stability-only for both tested attacks; collusion-only would
     additionally have caught `persistent_sybil` (mean 1.08 vs. the
     shipped variant's 16.99) — quantifies the AND-gate's conservatism
     cost precisely.
  2. **Solo (non-Sybil) attacker** (§5.7, Experiment 2.6, 1,000 rows,
     `experiment_2_6_solo_attacker.sh`): drops to 1 attacker, no
     colluding partner. DSS still helps (`dss_fedavg` 36.97 vs. plain
     `fedavg` 80.68) but far less than the 2-attacker case, and
     `dss_krum` regresses plain `krum` again (3.57 vs. 0.30) — a second,
     independent confirmation of Finding 3 above.
  3. **Mechanism analysis** (§5.8, using new `DssAggregator::
     last_diagnostics()` instrumentation — a pure, read-only diagnostic
     capture, never consulted by `aggregate()` itself — and a new
     `run_dss_diagnostics.rs` example): found a real, previously-unknown
     numerical implementation bug. When DSS wraps a fragile base
     (`fedavg`) under a solo attacker, the shared reference point
     itself gets dragged into instability, spuriously saturating
     *every* client's collusion score near ceiling (measured: 0.999998
     mean pairwise collusion among 9 honest clients, confirmed across 3
     seeds) — weights become tiny, floating-point-noise-dominated
     values that never hit the intended exact-zero fallback threshold,
     producing chaotic rather than predictable output. Concrete fix
     identified (epsilon-threshold fallback), not yet applied — tracked
     in §8.
  4. **Joint non-IID + attack** (§5.9, new `run_dss_diagnostics.rs
     --scenario joint`, 5 seeds, saved to `dss_diagnostics_joint.jsonl`):
     the still-open temporal-fairness scope note from §5.4/§6.4, now
     built and run. Finding: the joint protection claim (non-IID
     minority protected, attacker suppressed) holds *asymptotically* in
     every seed — but with a measured 6–13-round transient window where
     the legitimately non-IID, non-colluding minority is *also* wrongly
     zeroed alongside the real attackers, for the same shared-reference-
     instability reason as finding 3. The single most important
     qualification this session added to DSS's own hypothesis.
  Both new example runners (`run_experiment.rs`'s ablation prefixes,
  `run_dss_diagnostics.rs`) reuse the existing `Aggregator`/`Attack`
  trait surfaces unchanged — the only new production code is the
  additive `last_diagnostics()` method and its backing `Mutex` field on
  `DssAggregator`. 52 tests now passing in `conflux-core` (was 51, +1 for
  the new diagnostics accessor). Full workspace: 249 tests, `cargo fmt`
  and `cargo clippy --workspace --all-targets` both clean.

- **`cflux`/`cflux-dev` CLI design (2026-08-26)**: `docs/CLI_DESIGN.md`
  — a planning document, nothing built. Two binaries (production vs.
  research/testing), for the same reason `conflux-attacks` is a
  dev-dependency of nothing `conflux-server` ships (ADR 0010) — a single
  CLI binary with both `server start` and `experiment run` subcommands
  would quietly undo that guarantee. Full command tree (`cflux init`/
  `doctor`/`server`/`node`/`allowlist`/`checkpoint`;
  `cflux-dev experiment`/`aggregator`/`selector`/`privacy`/`attack`) and
  a command-by-command comparison against Flower's real, current `flwr`
  CLI (fetched live from `flower.ai/docs`, not from memory — their CLI
  turned out considerably larger than expected: app publishing to
  Flower Hub, federation/invitation management, a hosted chat agent).
  The comparison's conclusion: most of that extra surface exists to
  support things Conflux FL deliberately doesn't do (a hosted
  multi-tenant registry, personal-account login, multi-run-per-server
  job submission) — each traces back to an already-documented design
  choice (ADR 0003's no-multi-tenancy, the self-hosted posture), so the
  size difference mostly *confirms* those choices rather than surfacing
  real gaps. Two things flagged as genuinely worth adopting regardless:
  `--format json` on every command, not just some; and an open question
  (not answered) about whether a lighter-weight simulated-clients mode
  is worth building alongside the existing real-process e2e demos.

- **Phase 14 — `PerClient` epsilon accounting, shipped (2026-08-26)**:
  the first of the seven Part B items that had a ready-to-build phase
  brief. `RdpAccountant` (`conflux-privacy`) gained
  `record_round_for_client`/`current_epsilon_for_client`/
  `budget_exhausted_for_client`, sharing the exact same RDP composition
  math as `Global` scope (factored into one `epsilon_from_rounds`
  helper) — evaluated against a different history, never a different
  formula. `PrivacyRoundLog` (`conflux-store`) gained
  `append_round_for_client`/`load_client_rounds`, persisting raw
  `(noise_multiplier, sample_rate)` rounds per client in a new
  `PostgresStore` table — deliberately *not* a precomputed cumulative
  epsilon number (a real, deliberate deviation from the phase brief's
  literal schema, explained in the phase brief's own "Outcome" section
  and ADR 0006's second "Update"): a precomputed value is only valid
  for whatever `delta` computed it, and goes silently stale the moment a
  later run resolves a different one. `conflux-server`'s round pipeline
  moved the budget check from a pre-selection experiment-wide gate
  (`Global`, unchanged) to a post-decode per-client filter (`PerClient`,
  new) — `budget_exhausted_action = Halt` aborts the round the moment
  any one client is over budget, `ContinueWithoutGuarantee` excludes
  just that client and continues with everyone else. Both `RdpAccountant`
  and `PrivacyRoundLog` now always record/persist *both* scopes'
  history regardless of which is configured, so switching
  `accounting_scope` between restarts never silently loses whichever
  history wasn't active at the time — not specified by the brief,
  the more robust choice. `AccountingScope::PerClient` no longer fails
  fast at `resolve()` (ADR 0006's original fail-fast test inverted, as
  that ADR's own text predicted it would). Two new real end-to-end
  tests (a live gRPC server, two real client connections, one
  pre-exhausted) confirm both `budget_exhausted_action` outcomes;
  a real-Postgres restart-recovery test confirms per-client history
  survives a simulated restart. 249 → 260 tests passing workspace-wide
  (11 new); `cargo fmt` and `cargo clippy --workspace --all-targets`
  both clean.

## Stabilization (2026-08-31)

Auditing conflux-fl for a stable release turned up defects that 343
passing tests had not, because the tests exercised *plausible* batches
and a `ClientDelta` arrives from the network.

**Tier 1 — remotely-triggerable, all fixed:**

- **Non-finite weights crashed or corrupted every aggregator.** One
  client sending `NaN` — four bytes — made six aggregators *panic*
  (`krum`, `multi_krum`, `trimmed_mean`, `median`, `bulyan`,
  `median_of_means`, all via `partial_cmp(...).expect("never NaN")`),
  taking the server down; the other six returned `NaN`, which lands in
  the checkpoint and ends the experiment silently. Now rejected at
  `decode_and_validate` — the single chokepoint all eleven aggregator
  entry points share — as `AggregatorError::NonFiniteWeights`, naming
  the client and the coordinate.
- **`num_samples` was unbounded.** A client claiming `u64::MAX` samples
  made FedAvg's output *exactly* its own submission, every honest
  contribution numerically erased. Now bounded by
  `MAX_PLAUSIBLE_SAMPLE_COUNT` (2^40). Note this closes the degenerate
  case only: no absolute ceiling distinguishes a liar claiming 100,000
  from a genuinely large client, and the real defenses remain a robust
  aggregator (`krum` was unharmed throughout) or not accepting
  unauthenticated counts.
- **`geometric_median` overflowed to infinity on finite input** — found
  by the new adversarial suite on its first run. It multiplied by raw
  sample counts before normalizing, so `10 * f32::MAX` reached infinity
  before the division that would have brought it back.
  `WeightedAverageAggregator` already normalized first; this now does
  too, in both the initialization and the Weiszfeld iteration. Same
  formula, different order of operations.
- **New `crates/conflux-core/tests/adversarial_input.rs`** (12 tests):
  every aggregator against `NaN`, both infinities, a single bad
  coordinate, `f32::MAX`, denormals, zero and impossible sample counts,
  empty batches, single clients, `dim = 1`, mismatched dimensions, and
  truncated bytes. The rule it encodes: **an aggregator may reject a
  batch and may return a defensible number, but must never panic and
  must never return a non-finite value.**
- **The HTTP admin API had no authentication at all**, while the gRPC
  surface beside it had two layers. `/admin/allowlist` decides who may
  participate, so an unauthenticated write there undid the gRPC port's
  authentication entirely; `/clients/register` separately bypassed both
  the JWT check and the allow-list. Now behind `CONFLUX_ADMIN_TOKEN`
  (constant-time comparison, `/health` exempt), and **binding beyond
  loopback without a token refuses to start** — verified against the
  real binary in all three states.

**Tier 2 — all fixed:**

- `FileStore`'s synchronous `std::fs` calls now run on
  `spawn_blocking`. They were occupying tokio executor threads, so a
  slow disk during a checkpoint write stalled the gRPC service and the
  round timer along with it.
- `S3Store::connect` now checks `head_bucket` before `create_bucket`,
  instead of issuing a write request on every connection — which also
  means credentials scoped to object read/write no longer fail a
  permission they never needed.
- `clip_radius = 1.0` is documented as a **placeholder, not a default**,
  in the config field, the aggregator, `USAGE.md`, and — most usefully —
  a startup warning that fires when `centered_clipping` is selected with
  an untuned radius. Verified to fire only in that case. The default
  itself is unchanged: `τ` bounds an L2 norm in parameter space, so no
  value is right for an unknown model (§5.13).

## Tier 3 — release engineering (2026-08-31)

- **S10 — workspace manifest.** `[workspace.package]` and
  `[workspace.dependencies]`. `version`/`edition`/`license`/
  `repository`/`rust-version` are declared once; 32 third-party
  versions are declared once. Features stay per-crate deliberately —
  `conflux-store` needs tokio's `rt` and nothing else, and hoisting the
  union would compile capabilities into crates with no use for them.
- **S9 — Apache-2.0.** `LICENSE` (canonical text), `license`/
  `description`/`repository` on all 13 crates, a README license section.
  `cargo package` now succeeds. `conflux-attacks` is marked
  `publish = false` — ADR 0010 forbids the dependency edge that
  publishing it would invite.
- **S11 — MSRV, measured rather than guessed.** `rust-toolchain.toml`
  pins the channel. `rust-version = 1.85` (edition 2024's floor) for
  eleven crates; `conflux-store` and `conflux-server` declare **1.94.1**
  because `aws-sdk-s3` requires it — found with `cargo metadata`, not
  assumed. Declaring an MSRV immediately paid for itself:
  `clippy::incompatible_msrv` caught `decode_weights` using
  `usize::is_multiple_of` (stable 1.87), which would have broken the
  1.85 promise. Rewritten as `% 4 != 0`.
- **S12 — reproducible backends.** `docker-compose.yml` on the same
  non-standard ports `USAGE.md` already documented, so existing dev
  containers keep working. The seven hardcoded backend URLs became
  env-configurable (`CONFLUX_TEST_REDIS_URL` and friends) — which is
  what let CI point them at its own service containers. Verified the
  override takes effect by aiming one at a dead port.
- **S12 — env configuration via [evnx](https://evnx.dev).**
  `.env.example` documents all 32 variables; `.env` is gitignored.
  Optional variables are **commented out rather than set empty**: unset
  lets `conflux-config` resolve through its chain and log the source
  (ADR 0007), while empty is an explicit override to nothing. That
  distinction is also what took `evnx validate` from 10 errors to 0.
- **S8 — CI.** Five jobs: `fmt`, `clippy` (`--all-targets`, warnings
  denied), `test` (with Redis/Postgres/MinIO service containers, so the
  three durable backends are actually exercised), `msrv` (checks the
  1.85 crates on a 1.85 toolchain), and `env-files` (`evnx scan` plus a
  direct `git ls-files .env` guard).

Every job was verified locally before being written down, and doing so
found three things: the `is_multiple_of` MSRV violation, an unindented
helper that `cargo fmt --check` rejected, and an `evnx scan .env.example`
step that was scanning **zero files** because evnx excludes that filename
by design. The scan now covers the whole checkout, validated against a
simulated CI tree (265 files, no `.venv`, no `.env`, 0 high-confidence
findings).

## Tier 4 — API stability for downstream (2026-08-31)

The tier `conflux-web` and the DSS research line actually needed:
both build on these crates, and "whatever compiles today" is not
something either can plan against.

- **S14 — the public API is documented.** `#![warn(missing_docs)]` on
  all thirteen crates, and the 341 undocumented public items it found
  are gone. Worst offenders were `conflux-config` (107),
  `conflux-server` (57), and `conflux-core` (39). The lint is the
  enforcement: an undocumented public item is now a build warning, and
  CI denies warnings, so this cannot silently regress. Doc comments on
  `.proto` fields propagate into the generated Rust, so the schema
  shared with Python was documented at the source rather than in the
  codegen output.
- **S15 — public API review**, written up as
  [`docs/API_STABILITY.md`](API_STABILITY.md). States the promise
  (`0.x`: breaking changes land in minor versions and are listed here —
  a commitment to *disclosure*, not stability), why not `1.0` yet, what
  is deliberately **excluded** from the promise (`conflux-attacks`
  entirely; `conflux-core`'s `DssAggregator`/`ClientDssDiagnostic`, an
  unvalidated research hypothesis), and per-layer settledness — the wire
  contract is the most stable, `AppState` the least.
- **S16 — a runnable example for each of the four crates that never had
  one.** Each is a real "try it", not a snippet:
  - `conflux-core/examples/compare_aggregators.rs` — twelve methods on
    one poisoned batch, plus every typed rejection path.
  - `conflux-node/examples/local_hop.rs` — pull mode, push mode, and the
    client-side privacy transform over a real local gRPC hop.
  - `conflux-server/examples/round_pipeline.rs` — one complete round
    end to end: real `AppState`, real gRPC, real clients, `run_round`
    driving the actual pipeline. Nothing mocked.
  - `conflux-attacks/examples/attack_vs_defense.rs` — the attack ×
    defense matrix.

  Writing them found two things prose alone would not have. The server
  example's checkpoints came out looking random until the default
  privacy transform (`gaussian_clipping`, `clip_norm = 1.0`,
  `noise_multiplier = 1.0`) was explicitly disabled in it — a good
  demonstration that the default is on, and a bad one for showing what
  aggregators do. And an error message's formatting bug surfaced only
  when a human-readable example printed it.
- **S17 — catalog drift closed.** `AGGREGATION_LANDSCAPE.md` gained its
  "Update (2026-08-30, fifth) — Centered Clipping shipped" section. The
  web-side catalog remains a `conflux-web` concern.

**One CI-breaking defect fixed after the fact**: `attack_vs_defense.rs`
carried a `clippy::useless_vec` warning. Locally harmless; CI runs
`clippy --all-targets -- -D warnings`, so it would have failed the first
run against the pushed branch. Verified clean now, along with `cargo fmt
--check` and the full 367-test suite against real Redis, Postgres, and
MinIO backends (0 ignored).

## Tier 5 — production hardening (2026-08-31, COMPLETE)

A post-Tier-4 audit for what "stable" should mean beyond "compiles,
tested, documented". Three defects, none caught by 367 tests, all in
the same class as Tier 1's: they are about what the process does when
something goes wrong, which the tests never exercise because they only
ever exercise the path where nothing does.

- **H1 — unbounded chunk accumulation (remotely triggerable).**
  [`service.rs`](../crates/conflux-net/src/service.rs)'s `submit_delta`
  collects a client-controlled stream into a `Vec` with no cap on
  chunk count, before any dispatcher-level check runs. tonic's default
  4 MiB limit is **per message**, not per stream, so `N` chunks are
  `N × 4 MiB` of server memory. One client that never stops sending
  exhausts the heap. Same class as Tier 1's non-finite weights — a
  trust-boundary input with no bound on it — and the fix is the same
  shape: reject past a configured maximum, with a typed error naming
  the client. Needs a `max_update_bytes` (or `max_chunks`) config key,
  which is a `conflux-config` addition, not just a `conflux-net` one.

- **H2 — the round loop dies silently, and `/health` lies about it.**
  [`main.rs`](../crates/conflux-server/src/main.rs)'s loop `break`s on
  every error except `EmptyBatch`. A transient Redis blip or a Postgres
  reconnect therefore stops the experiment permanently — while the gRPC
  and HTTP servers keep running, so the process stays up and
  [`http.rs`](../crates/conflux-server/src/http.rs)'s `/health` keeps
  returning a hardcoded `"ok"`. An orchestrator sees a healthy pod
  doing no work, indefinitely, with one `tracing::error!` line as the
  only evidence. Two separable fixes: retry-with-backoff for transient
  backend errors (keeping genuinely fatal ones fatal — an exhausted
  privacy budget *should* stop the loop), and a `/health` that reports
  round-loop liveness instead of a constant.

- **H3 — no graceful shutdown.** No `SIGTERM` or `ctrl_c` handling in
  either binary. `docker stop`, a Kubernetes eviction, or Ctrl-C kills
  the server mid-round: submissions already buffered are lost, and a
  checkpoint being written has no chance to finish. `run_round` is
  already the natural boundary — draining to the end of the current
  round, refusing new work, then exiting is the shape.

**All three shipped.** 20 new tests (367 -> 387), clippy clean under
`-D warnings`, fmt clean.

- **H1 — fixed.** `max_update_bytes` is a real `conflux-config`
  parameter (builtin 256 MiB, `CONFLUX_MAX_UPDATE_BYTES`, experiment-file
  key, logged with its source like every other value per ADR 0007). It
  carries a builtin and is owned by *neither* axis — `None` for both
  topology and mode — which keeps ADR 0001's disjointness intact: a
  payload ceiling is a framework safety bound, not a claim about what
  kind of participants a deployment has. `submit_delta` now counts bytes
  as each chunk arrives and **before** pushing it, so the peak is one
  chunk over the limit rather than whatever the client felt like
  sending, and returns a typed `DispatchError::UpdateTooLarge {
  client_id, limit_bytes, received_bytes }` mapping to gRPC
  `resource_exhausted` (8) — not `invalid_argument`, because the request
  was well-formed and the server simply refused to hold that much of it.
  The client id is taken from the chunk in hand and is therefore
  *claimed, not verified*: the bound is deliberately enforced ahead of
  any identity check, since a check that ran first would be the thing
  being flooded. Documented as such on the variant. `conflux-net` gained
  a `tracing` dependency for the rejection log, per the same "say so,
  out loud" principle `conflux-buffer` and `conflux-reputation` already
  follow.
- **H2 — fixed, and the decision is recorded.** `ServerError::
  is_transient` draws the line, and the question it answers is not "how
  bad is this error" but **"can the next round differ from this one?"**
  Backend I/O (`Registry`, `Store`) and *every* aggregation rejection
  are transient — a rejected batch is a statement about this round's
  batch, and the client that sent a `NaN` may not be selected next
  round. An exhausted privacy budget, in either scope, is fatal: `halt`
  means halt and no amount of waiting produces more epsilon (ADR 0006).
  The loop backs off exponentially (2s doubling to a 60s cap) and races
  the sleep against shutdown, so Ctrl-C during a 60-second backoff does
  not wait it out.
- **H2 — `/health` reports the loop.** New `round_health.rs`:
  `starting` / `running` / `degraded` / `stopped`, with the last
  completed round, the consecutive-failure count, and the last error.
  **`degraded` returns 200 deliberately** — a loop retrying an
  unreachable Redis is alive, and failing its health check would turn a
  backend outage into a crash loop on top of a backend outage.
  `stopped` returns **503**, because that is the state a restart or a
  config change is the only remedy for. Read through atomics rather than
  one mutex: a health check that can be blocked by the thing it is
  checking is worse than useless.
- **H3 — fixed, in both binaries.** `SIGTERM` and Ctrl-C are handled via
  a `watch` latch (not `broadcast` — a late subscriber must still see
  that shutdown was requested). The server's gRPC and HTTP listeners
  stop accepting; the round loop checks the latch **between rounds and
  never during one**, so a shutdown arriving mid-round waits for that
  round rather than abandoning buffered submissions and a half-written
  checkpoint. `conflux-node` gained the same handling — it has no round
  state to drain, but its local listener now closes rather than the
  process vanishing, so a Python `ClientApp` mid-call sees a closed
  connection instead of a reset.

**Each fix was verified to fail without itself**, not just to pass with
it. Neutralising H1's bound turns two of its four tests red; a process
with no `SIGTERM` handler exits `-15`, which is not `success()`, so H3's
subprocess tests go red too. H3's tests drive the **real binary** — signal
handling is process-level behavior, and the wiring between handler,
listeners, and round loop is exactly what a library-level test would
miss.

**One thing the tests do not prove.** H3's "drain the round in flight"
property is structural, not asserted: the loop awaits `run_round` as a
unit with no `select!` around it, so it *cannot* abandon a round
mid-flight. Observing that from outside the process would need a round
with a real submission in it and instrumentation to catch the moment.
Worth building if the loop ever gains a cancellation path.

### Breaking change: the `/health` response shape

`GET /health` returned the bare string `ok`. It now returns JSON:

```json
{"status":"ok","round_loop":"running","last_completed_round":12,
 "consecutive_failures":0}
```

`status` is kept as the first field with `"ok"` as its value, so a naive
substring match still works — but anything comparing the whole body to
`ok` will break, and `tests/integration.rs`'s own health test did. Listed
here per `API_STABILITY.md`'s promise that breaking changes are disclosed
in `STATUS.md`. The bar that justifies it: the previous shape could not
express the failure it was being polled to detect.

## Research-line entry point

The DSS research now has its own harness (scaffolded 2026-08-31 with the
`research-harness` skill, `.claude/skills/research-harness/`):

- [`docs/research/AGENT.md`](research/AGENT.md) — entry point: what the
  project is, where each kind of context actually lives, how to run an
  experiment, the constraints, and a self-check protocol.
- [`docs/research/PROGRESS.json`](research/PROGRESS.json) — session
  handoff state, including a `conclusions_overturned_by_measurement`
  list.
- [`docs/research/tasks.json`](research/tasks.json) — 10 atomic tasks,
  each with a `done_when`.
- [`docs/research/BASELINES.md`](research/BASELINES.md) — every reference
  number consolidated, each copied from a named `results/*.summary.csv`.

Deliberately *not* created: `EXPERIMENT_LOG.md`, `LITERATURE.md`,
`DECISIONS.md`, `RESEARCH_PLAN.md`, `ARCHITECTURE.md`. All five already
exist as sections of `temporal-consistency-aggregation.md`, the ADRs, or
`docs/ARCHITECTURE.md`; duplicating them into thinner files would
produce drift, not clarity. `AGENT.md` maps each one to where it lives.

## In progress
(none)

## Next

**Stabilization: Tiers 1–5 are complete — `conflux-fl` is stable.**
What's left on the framework line, in the order it makes sense to take
it:

- **Tier 5 — feature gaps**, all deferred past "stable" deliberately:
  ADR 0012's stateful-aggregator proto extension (unlocks
  FedNova/SCAFFOLD/FedOpt together) · ADR 0011's trusted-reference
  sidecar · ADR 0005's Python SDK interface question · Phase 21's
  profile-file `inherits` · a `cflux` CLI.
- **Publishing (P1–P3), gated on a decision that hasn't been made.**
  Whether crates.io publishing is intended is still open. If yes:
  **P1** every internal path dependency needs a `version` alongside its
  `path` (`cargo package -p conflux-core` fails today without it);
  **P2** decide facade vs. no facade — twelve crates published
  separately keeps the layering that lets a researcher take
  `conflux-core` (77 transitive deps) without `conflux-server`'s 264,
  and a thin `conflux-fl` facade would both give one dependency line
  and reserve the name; **P3** `cargo publish --dry-run` in dependency
  order (wave 1: `config`/`proto`/`registry`/`reputation`/`store`;
  wave 2: `buffer`/`core`/`net`/`privacy`/`selector`; wave 3: `node`,
  then `server`). If publishing is *not* intended, P1–P3 all drop and
  the metadata still stands on its own for the license.

Then `conflux-web`, then the DSS research line below.

**DSS paper (2026-08-31).**
[`docs/research/paper/dss.tex`](research/paper/dss.tex) — a standalone
LaTeX write-up of the research line as it stands: title, abstract,
introduction, related work, problem formulation, the proposed method,
experimental setup, results, a dedicated ablation section, real-data
validation, and a section for the four conclusions measurement
overturned. Self-contained (inline `thebibliography`, no `.bib` or
bibtex pass); figures resolve to `docs/research/figures/`. **Not
compiled** — no TeX toolchain on this machine. This partly overlaps
research task `p3` (extract a standalone results narrative), which was
blocked on `p1`, the publication decision; the paper exists now either
way, and `p1` still governs whether it's worth polishing toward a venue.

---

Every item from `docs/research/temporal-consistency-aggregation.md`'s
original validation plan (§7.1, now 8 items) is done, including DSS
itself, its mechanism ablation, its solo-attacker generalization, and the
temporal-fairness-under-attack experiment. What remains, per the user's
own combined task list plus the 2026-08-24 novelty-positioning follow-up:

**Research (Part A)**
1. ~~**Fix §5.8's numerical bug**~~ — **done (§5.8.1)**, with a
   different fix than the one prescribed and the opposite conclusion.
   The suggested `1e-4 * n` epsilon fallback is *measurably wrong*: it
   fires on a case with clean, correct discrimination (an existing unit
   test's `weight_sum` is `1.38e-4` at `n = 4`) and replaces the honest
   consensus with the sybil-dominated mean. The real defect was
   catastrophic cancellation in `1 − collusion` computed in `f32`, fixed
   by computing the collusion score in `f64`. **Fixing it did not fix
   the symptom**: cross-seed variance halved (CV 1.03 → 0.43) so results
   are now reproducible, but the solo-attacker failure remains and the
   §5.9 joint transient window is *byte-identical*. The two did not
   share a root cause after all. The real cause is §5.8's own point 1 —
   an unstable shared reference — which is a design problem, not
   arithmetic.
2. ~~**Fix DSS's Finding 3**~~ — **done (§5.11, Experiment 2.8, 3,600
   rows)**. DSS now applies its judgment *through* the base method
   (drop fully-distrusted clients, scale survivors' `num_samples`, call
   `base.aggregate`) instead of combining the raw batch itself. All nine
   robust-base cells fixed: `dss_krum` vs `persistent_sybil` 16.99 →
   0.297, `dss_krum` vs `scaling` 171.47 → 0.297 (a 577× regression,
   gone), `dss_multi_krum` 16.99 → 0.173. The one configuration DSS
   genuinely helps is untouched (`dss_fedavg` 1.175 → 1.178). Experiments
   2.4 and 2.6 re-run and confirm. **§5.5's "DSS-on-`fedavg` only"
   recommendation is withdrawn** — that existed solely because of this
   defect.
3. ~~**A harder synthetic collusion model**~~ — **done (§5.12,
   Experiment 2.9, 1,800 rows)**. `CorrelatedSybilAttack` — colluders
   sharing an objective but each with its own fixed offset, so they are
   correlated, non-identical, and temporally *stable*
   (`divergence = 0` reproduces `persistent_sybil` exactly, so it is a
   strict generalization). **§5.6's open question is answered: the
   collusion signal is not redundant.** Collusion-only catches these
   (1.09) where stability-only misses them entirely (17.13, no better
   than undefended `fedavg`) — ~15× apart, so §5.6's "numerically
   identical" was an artifact of its attack model. Separately and
   unplanned: **FoolsGold degrades 5.6× against non-identical colluders**
   (1.35 → 7.54 → 9.93), a real limitation of the published method under
   a threat model its own paper doesn't test, while DSS's
   deviation-trace collusion signal holds at 1.09 — turning §6.5's
   architectural distinction between the two into a measured one.
4. ~~**CIFAR-10 / FEMNIST / Shakespeare dataset harnesses**~~ — **mostly
   done**. CIFAR-10 already existed. New: `e2e_pytorch_shakespeare` — a
   character-level GRU with **one client per speaking role**, so the
   non-IID-ness is natural rather than a Dirichlet knob, and the task is
   sequence modelling rather than a fourth image classifier (verified
   end-to-end: 0.017 → 0.171 held-out accuracy over 5 rounds against a
   0.204 centralized baseline, chance 1/65). `benchmark.py` gained an
   `--attacks` dimension — the gap that actually blocked using these
   harnesses for §5's questions rather than convergence demos.
   **FEMNIST deliberately deferred**: writer identity is absent from
   torchvision's EMNIST distribution, so building it from there would
   produce another synthetic partition, defeating the point; a faithful
   version needs LEAF's preprocessing over raw NIST SD19 (several GB).

   **The first real-data run already found something (§5.13).** On real
   MNIST with a real 50,890-parameter MLP: `krum` (0.844) and
   `trimmed_mean` (0.875) hold under attack where `fedavg` collapses
   (0.884 → 0.163) — §5's central synthetic finding, reproduced. But
   **Centered Clipping at its default `τ = 1.0` scores 0.078, worse than
   no defense**, and a τ sweep (1 → 5 → 20 → 100) rises monotonically
   toward FedAvg's own number with no optimum anywhere. §5.10 found a
   genuine τ optimum at `dim = 3`; τ bounds an L2 norm in parameter
   space, so it does not transfer across model sizes at all. The builtin
   `clip_radius = 1.0` is a placeholder, not a shippable default.

**Planning/design (Part B) — scoped 2026-08-23; implementation started
2026-08-26 and completed 2026-08-30. All seven ready-to-build phase
briefs are shipped.** What remains under Part B are the three items that
never had a buildable brief — ADR 0005's Python SDK interface question,
ADR 0011's trusted-reference sidecar, and ADR 0012's stateful-aggregator
proto extension:
- ~~[`docs/phases/phase-14-perclient-accounting.md`](phases/phase-14-perclient-accounting.md)~~
  — **shipped**, see the "Done" entry above.
  [`docs/adr/0006-global-epsilon-accounting.md`](adr/0006-global-epsilon-accounting.md)
  now has a second "Update" section recording it.
- [`docs/adr/0005-python-sdk-deferred.md`](adr/0005-python-sdk-deferred.md)'s
  2026-08-23 "Update" section — Python SDK, decomposed into three
  separable questions (model handoff, code distribution, `ClientApp`
  interface); recommends resolving the interface question first since
  it's the only one this codebase can answer alone.
- [`docs/adr/0011-server-trusted-reference-boundary.md`](adr/0011-server-trusted-reference-boundary.md)
  (new) — FLTrust/Zeno's server-training requirement vs. ADR 0004;
  recommends an optional sidecar process rather than a server-binary
  training dependency.
- ~~[`docs/phases/phase-15-centered-clipping.md`](phases/phase-15-centered-clipping.md)~~
  — **shipped**. `centered_clipping` in the catalog; Experiment 2.7
  (3,000 rows) placed it against the other cross-round methods and
  measured its `τ` sensitivity.
- [`docs/adr/0012-stateful-aggregator-and-proto-extension.md`](adr/0012-stateful-aggregator-and-proto-extension.md)
  (new) — the shared plumbing FedNova/SCAFFOLD/FedOpt all need: keeps
  `Aggregator::aggregate`'s `&self` signature, adds two `optional`
  `ClientDelta` fields additively.
- ~~[`docs/phases/phase-16-jwt-auth-verification.md`](phases/phase-16-jwt-auth-verification.md)~~
  — **shipped**. RS256/ES256, algorithm pinned to the key (never the
  token header), `sub` bound to the registering client, and a new
  `DispatchError::Unauthenticated` so a bad credential is
  distinguishable from an uninvited one.
- ~~[`docs/phases/phase-17-client-side-privacy-transform.md`](phases/phase-17-client-side-privacy-transform.md)~~
  — **shipped**, off by default. Chunks are reassembled before clipping
  (clipping per chunk would make the guarantee depend on fragmentation)
  and the transform runs once before the retry loop, not per attempt.
- ~~[`docs/phases/phase-18-push-mode-node.md`](phases/phase-18-push-mode-node.md)~~
  — **shipped**. `cross_silo`'s own default posture (`push` + mTLS) now
  runs end to end, tested together for the first time.
- ~~[`docs/phases/phase-19-simd-aggregation.md`](phases/phase-19-simd-aggregation.md)~~
  — **shipped, with the opposite result to the one it assumed.** The
  benchmark it insisted on is what killed it: explicit SIMD measured
  *slower* at every realistic model dimension, because the loop is
  memory-bandwidth-bound and LLVM already auto-vectorizes it. The
  shared-helper refactor (8 duplicated loops → 1) shipped; the SIMD
  didn't. The benchmark stays as the standing answer.
- ~~[`docs/phases/phase-20-config-file-parsing.md`](phases/phase-20-config-file-parsing.md)~~
  — **shipped** (experiment-file half). `CONFLUX_EXPERIMENT_CONFIG_PATH`,
  flat TOML, `deny_unknown_fields` so a typo'd key is refused rather
  than silently dropped. Profile-file `inherits` remains a future
  Phase 21.

`docs/AGGREGATION_LANDSCAPE.md` gained a matching "Update (2026-08-23,
fourth)" section cross-linking the four aggregation-related documents
above from its own summary table.

`docs/AGGREGATION_LANDSCAPE.md`'s original trait-taxonomy gaps are now
closed — Geometric Median/RFA (whole-vector shape), a Bulyan-shaped
member, and DSS (the temporal-defense shape) all shipped this session
(see the "Done" entries above), leaving Centered Clipping and
FLTrust/Zeno as Part B's only remaining aggregation-taxonomy gaps (both
listed above). Also still open, not part of the user's current Part
A/Part B list: resource-aware/utility-based selectors, `libloading`-
based dynamic plugin loading, hierarchical topology. Fang et al.
(2020)'s optimization-based attack against Krum/Trimmed-Mean/Median
specifically, and a many-round/higher-dimensional attack/defense harness
(to actually observe ALIE's documented failure modes, which Phase 12's
single-round test didn't reproduce — this session's work also didn't run
ALIE specifically, only the other attacks), are natural
`conflux-attacks` follow-ups — see `docs/phases/
phase-12-attack-simulation.md`'s "Not in scope" note.

See "Known deviations from spec" below for JWT auth verification,
client-side privacy transform, push mode in `conflux-node`, SIMD
aggregation, and config-file parsing — each a larger, dedicated-
phase-sized feature also listed in Part B above, not a small fix.

## Known deviations from spec
- `conflux-proto` uses `tonic` + `tonic-prost`/`tonic-prost-build` rather
  than a single `tonic`+`prost` pairing — just tonic 0.14's naming.
- Spec §3's promised per-topology numeric defaults (beyond
  `round_timeout_secs = 300` for `cross_device`) are Phase 1 placeholders.
- Spec §11 Open Item 2: backend selection is resolved (Phase 8a,
  env-var driven, deliberately outside `conflux-config`'s `Overrides` —
  see that phase brief's scope note). **Experiment-file** parsing is
  now done too (Phase 20 — `CONFLUX_EXPERIMENT_CONFIG_PATH`, flat TOML
  into the `file` tier `resolve()` always had). **Profile-file**
  parsing — topology/mode profiles themselves defined in TOML with
  `inherits` extension, spec §4.1 — remains open, tracked as a future
  Phase 21.
- `auth`'s values are lowercase `mtls`/`jwt`.
- `conflux-privacy`'s `RdpAccountant` computes non-subsampled RDP — a
  conservative upper bound.
- `conflux-net`/`conflux-node`'s auth and privacy gaps are closed: mTLS
  (7e), JWT verification (Phase 16 — RS256/ES256, `sub` bound to the
  registering client), push mode in `conflux-node` (Phase 18 —
  `cross_silo`'s own default posture, `push` + mTLS, now runs end to
  end), and the client-side privacy transform (Phase 17 —
  `client_side_privacy_transform`, off by default).
- **New deviation (Phase 17)**: `conflux-node` now depends on
  `conflux-privacy` (and transitively `conflux-config`), not just
  `conflux-proto`/`conflux-net` as spec §2 describes. Deliberate —
  spec §8's sequence diagram requires the node to apply the mechanism,
  which is impossible without reaching it. `conflux-node` still calls
  no `conflux-config` API directly.
- `conflux-core`'s weighted-sum accumulation is a plain loop, not SIMD
  intrinsics — and this is now a **measured decision**, not an
  outstanding gap (Phase 19). Explicit `wide`-based `f32x8` SIMD was
  built and benchmarked against it: slower at every realistic model
  dimension (1.21 µs vs 1.35 µs at dim=10k; 145 µs vs 154 µs at
  dim=1M), because the loop is memory-bandwidth-bound and LLVM
  already auto-vectorizes it. The eight duplicated inline loops were
  still consolidated into one shared helper. `cargo bench -p
  conflux-core` reproduces the comparison.
- The `RoundBuffer` lost-update race is closed (Phase 10a).
- `conflux-config`'s `inventory` registry is wired for `aggregator`
  (`fedavg`/`krum`/`multi_krum`/`trimmed_mean`/`median`, Phase 10b/11a),
  `selector` (Phase 10b), and `privacy_mechanism` (Phase 11b) — all three
  spec §5 families, not two of three.
- `FileStore`'s internals are still blocking `std::fs` calls under an
  `async fn` signature (Phase 7b note, unchanged).
- ~~`S3Store`'s `create_bucket` call on every
  `connect`/`connect_with_prefix`~~ — **closed in Tier 2 (S7)**.
  `connect` now checks `head_bucket` first, which also means
  credentials scoped to object read/write no longer fail a permission
  they never needed. Left listed, struck through, because this entry
  outlived its fix once already.
