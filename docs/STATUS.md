# Conflux — Status

Last updated: 2026-08-26, Phase 14 (`PerClient` epsilon accounting) shipped — first of the seven ready-to-build Part B items; `docs/CLI_DESIGN.md` (a `cflux`/`cflux-dev` CLI proposal, full comparison against `flwr`) added as a new planning doc

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

## In progress
(none)

## Next
Every item from `docs/research/temporal-consistency-aggregation.md`'s
original validation plan (§7.1, now 8 items) is done, including DSS
itself, its mechanism ablation, its solo-attacker generalization, and the
temporal-fairness-under-attack experiment. What remains, per the user's
own combined task list plus the 2026-08-24 novelty-positioning follow-up:

**Research (Part A)**
1. **Fix §5.8's numerical bug** (route to the unweighted-mean fallback
   when `weight_sum` is below a small epsilon, e.g. `1e-4 * n`, not only
   exactly `0.0`) and re-run Experiments 2.6 (§5.7) and the joint
   diagnostics (§5.9) to see whether it shortens or removes the
   transient false-positive window both share a root cause with —
   priority order per the doc's own "Recommended order" (§8): cheap,
   mechanical, and worth doing before more design work sits on top of a
   known bug.
2. **Fix DSS's Finding 3** (combine step should blend the base method's
   own selection into the final weights, not just measure deviation
   against its output) and re-run Experiments 2.4/2.6 to confirm — OR
   scope DSS to `fedavg`-only use until that redesign happens (now
   confirmed in two independent scenarios, §5.5 and §5.7).
3. **A harder synthetic collusion model** (correlated but non-identical
   Sybils) — §5.6's ablation used identical-submission Sybils, which
   can't test whether the collusion signal adds independent value beyond
   stability alone; a harder model could.
4. **CIFAR-10 / FEMNIST / Shakespeare dataset harnesses** — not started.
   Deliberately last per the original plan (expensive relative to what
   they'd add), and now more valuable once 1–3 narrow DSS's remaining
   open questions first.

**Planning/design (Part B) — scoped 2026-08-23, all 10 topics have a
planning document; implementation started 2026-08-26 (1 of 7 ready-to-build
phase briefs shipped so far)**:
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
- [`docs/phases/phase-15-centered-clipping.md`](phases/phase-15-centered-clipping.md)
  — buildable now, no proto change needed, `temporal.rs`'s `Mutex`
  pattern is the precedent.
- [`docs/adr/0012-stateful-aggregator-and-proto-extension.md`](adr/0012-stateful-aggregator-and-proto-extension.md)
  (new) — the shared plumbing FedNova/SCAFFOLD/FedOpt all need: keeps
  `Aggregator::aggregate`'s `&self` signature, adds two `optional`
  `ClientDelta` fields additively.
- [`docs/phases/phase-16-jwt-auth-verification.md`](phases/phase-16-jwt-auth-verification.md)
  — mirrors Phase 9a's `resolve_server_tls` pattern; RS256/ES256 via
  `jsonwebtoken`, orthogonal to Phase 8c's `SharedToken` allow-list check.
- [`docs/phases/phase-17-client-side-privacy-transform.md`](phases/phase-17-client-side-privacy-transform.md)
  — reuses `GaussianClippingPrivacy::transform` unchanged from
  `conflux-node`, gated by a new `client_side_privacy_transform` toggle.
- [`docs/phases/phase-18-push-mode-node.md`](phases/phase-18-push-mode-node.md)
  — closes the gap where `cross_silo`'s own default configuration
  (`push` + mTLS) can't currently run end-to-end.
- [`docs/phases/phase-19-simd-aggregation.md`](phases/phase-19-simd-aggregation.md)
  — the `wide` crate, one shared `accumulate_weighted` helper covering
  every family member's combine step, with a criterion benchmark to
  actually measure the claimed speedup rather than assume it.
- [`docs/phases/phase-20-config-file-parsing.md`](phases/phase-20-config-file-parsing.md)
  — the experiment-level half only (`resolve()`'s `file` parameter is
  already fully plumbed and tested, just never fed a real parsed file);
  profile-file `inherits` semantics explicitly deferred to a future
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
