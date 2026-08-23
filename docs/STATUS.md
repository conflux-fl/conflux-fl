# Conflux — Status

Last updated: 2026-08-22, project renamed Confluo → Conflux (see ADR 0009's "Update" section)

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

## In progress
(none)

## Next
The reputation/aggregation pipeline-order gap above is the highest-value
next item — it's a real, currently-open weakness discovered by this
session's own E2E testing, not a hypothetical.
`docs/AGGREGATION_LANDSCAPE.md` (2026-08-22) generalizes it against ~18
real aggregation methods to inform the fix's design before it's scoped: a
batch-derived reference (even a robust one, like coordinate-wise median)
still has a Byzantine-fraction breakdown point, while FLTrust/Zeno's
"independent trusted reference" pattern doesn't — worth designing this
fix around a reusable trusted-reference primitive rather than just a more
robust batch statistic. That doc's exact call site:
`crates/conflux-server/src/round.rs:72`'s `mean_vector(&decoded)`, fed
into `conflux_reputation::filter_by_threshold` — `ContributionScorer`'s
own trait signature already takes `reference` as a caller-supplied
argument, so the fix is scoped to that call site, not a
`conflux-reputation` interface change. Candidate fixes (not yet designed
in detail): a trusted server-held reference (Category 3 of that doc); a
robust reference point (e.g. coordinate-wise median) as a lighter-weight
interim step; reputation scoring after aggregation instead of before; or
making reputation filtering off-by-default for deployments relying on
`robust` aggregation. That same doc also flags two trait-taxonomy gaps
worth resolving alongside this fix — Geometric Median/RFA needs a
whole-vector (not per-coordinate) robust-statistic shape, and Centered
Clipping needs cross-round aggregator state the current `Aggregator`
trait doesn't support — plus a proto-schema note: FedNova and SCAFFOLD
(popular, non-robustness-related methods) need new `ClientDelta`/
`DeltaChunk` fields, not just new trait impls, when they're eventually
built. Also
still open: a Dirichlet non-IID run of both E2E harnesses (both
`partition_data.py` scripts support `--split dirichlet`, not yet
exercised live); `PerClient` accounting (ADR 0006 — gated on per-client
round history landing in `conflux-registry`/`ExperimentStore`),
resource-aware/utility-based selectors, resolved Python SDK,
`libloading`-based dynamic plugin loading, hierarchical topology. A
future Bulyan-shaped `robust` member (El Mhamdi, Guerraoui & Rouault,
2018) now composes as `FilteredAggregator<BulyanFilter, TrimmedMean>`
with zero new plumbing, per Phase 11a's redesign. Fang et al. (2020)'s
optimization-based attack against Krum/Trimmed-Mean/Median specifically,
and a many-round/higher-dimensional attack/defense harness (to actually
observe ALIE's documented failure modes, which Phase 12's single-round
test didn't reproduce — this session's E2E work also didn't run ALIE
specifically, only the four attacks' simpler cousins), are natural
`conflux-attacks` follow-ups — see `docs/phases/
phase-12-attack-simulation.md`'s "Not in scope" note. Also
still open:
- JWT auth verification, client-side privacy transform, push mode in
  `conflux-node`, SIMD aggregation, and config-file parsing remain
  unimplemented (see "Known deviations from spec" below) — each is a
  larger, dedicated-phase-sized feature, not a small fix.

## Known deviations from spec
- `conflux-proto` uses `tonic` + `tonic-prost`/`tonic-prost-build` rather
  than a single `tonic`+`prost` pairing — just tonic 0.14's naming.
- Spec §3's promised per-topology numeric defaults (beyond
  `round_timeout_secs = 300` for `cross_device`) are Phase 1 placeholders.
- Spec §11 Open Item 2: backend selection is now resolved (Phase 8a,
  env-var driven, deliberately outside `conflux-config`'s `Overrides` —
  see that phase brief's scope note). Config-*file* parsing (vs. today's
  env-var/CLI-only `Overrides` sources) is still unresolved.
- `auth`'s values are lowercase `mtls`/`jwt`.
- `conflux-privacy`'s `RdpAccountant` computes non-subsampled RDP — a
  conservative upper bound.
- `conflux-net`/`conflux-node` don't implement JWT auth (mTLS now done,
  7e), client-side privacy transform, or push mode in `conflux-node`.
- `conflux-core`'s weighted-sum accumulation is a plain loop, not SIMD
  intrinsics.
- The `RoundBuffer` lost-update race is closed (Phase 10a).
- `conflux-config`'s `inventory` registry is wired for `aggregator`
  (`fedavg`/`krum`/`multi_krum`/`trimmed_mean`/`median`, Phase 10b/11a),
  `selector` (Phase 10b), and `privacy_mechanism` (Phase 11b) — all three
  spec §5 families, not two of three.
- `FileStore`'s internals are still blocking `std::fs` calls under an
  `async fn` signature (Phase 7b note, unchanged).
- `S3Store`'s `create_bucket` call on every `connect`/`connect_with_prefix`
  is a minor inefficiency (checks/creates on every connection, not just
  the first) — harmless (idempotent, MinIO/S3 both handle the
  already-exists case cheaply) but worth trimming if `S3Store` sees
  frequent reconnects in practice.
