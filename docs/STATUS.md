# Conflux — Status

Last updated: 2026-09-01 — **stabilization Tiers 1–6 complete, ADR 0011/0012 built, and the `optimization` family shipped**. Three remotely-triggerable defects fixed, the admin API authenticated, the project made releasable (Apache-2.0, workspace-inherited metadata, declared MSRVs, a compose file, evnx-managed env config, CI), the public API documented, reviewed, and demonstrated, the three production-hardening defects a post-Tier-4 audit found closed, and — Tier 6 — four more found by testing the stateful aggregators *across rounds*, which nothing had done: `centered_clipping` could be driven to a permanently `NaN` reference by one finite update, and a client could evade DSS's stability gate by submitting larger ones. Then the two deferred plumbing ADRs: 0012's optional proto fields (unblocking FedNova/SCAFFOLD/FedOpt) and 0011's trusted-reference sidecar, which makes **FLTrust** the first method in the catalog able to resist a colluding *majority*. Then the whole `optimization` family — FedAvgM, FedAdagrad, FedAdam, FedYogi and q-FedAvg — closing the framework's largest catalog gap, and **FLANDERS** — implemented to compare DSS against its closest published prior art, which found that DSS beats it ~15× on the adaptive attacker and that FLANDERS scores worse than undefended FedAvg against stable Sybils. **19 aggregation methods across five families.** Then Phase 23: the `ClientApp` SDK — ADR 0005's question (3), resolved in Python *and* in Rust, which is what finally lets a client populate ADR 0012's fields at all and takes FedNova/SCAFFOLD/FedProx/q-FedAvg from blocked to buildable. `crates/conflux-client` proves the loop closes with no Python process anywhere (0.67 local-only → 0.996 federated, on a problem no client can solve alone), and needed no server, node, or proto change to do it. Adding it to CI turned up a false published claim: the declared MSRV of 1.85 **did not build eight of the twelve crates that promised it** — corrected to 1.88. 490 tests (23 doc-tests, up from zero), clippy clean under `-D warnings`, fmt clean. Fifteen crates. Version 0.2.0.

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
  1.85 promise. Rewritten as `% 4 != 0`. **Superseded 2026-09-01: the
  1.85 figure was wrong** — `cargo metadata` reports what a crate
  *declares*, and eight crates' dependencies declare 1.88. Corrected to
  1.88; see phase 23. "Measured rather than guessed" was true of the
  1.94.1 overrides and false of the number it was written about.
- **S12 — reproducible backends.** `docker-compose.yml` on the same
  non-standard ports `USAGE.md` already documented, so existing dev
  containers keep working. The seven hardcoded backend URLs became
  env-configurable (`CONFLUX_TEST_REDIS_URL` and friends) — which is
  what let CI point them at its own service containers. Verified the
  override takes effect by aiming one at a dead port.
- **S12 — env configuration via [evnx](https://evnx.dev).**
  `.env.example` documents all **43** variables the code reads; `.env`
  is gitignored. Optional variables are **commented out rather than set
  empty**: unset lets `conflux-config` resolve through its chain and log
  the source (ADR 0007), while empty is an explicit override to nothing.
  That distinction is also what took `evnx validate` from 10 errors to 0.

  **Corrected 2026-08-31**: this said "all 32", and it was wrong — the
  binaries read 43. The ten undocumented ones were found by diffing
  `grep -rhoE '"CONFLUX_[A-Z0-9_]+"' crates/*/src/` against the file
  rather than by reading it, which is the only way this stays true.
  The omission mattered more than a count: `CONFLUX_REGISTRY_BACKEND`,
  `CONFLUX_STORE_BACKEND`, and `CONFLUX_ACCOUNTING_PERSISTENCE` are the
  three switches `validate_production_backends` fails fast without, so
  a deployer who copied `.env.example` to `.env` got a file that could
  not start a production server — the exact task the file exists for.
  Also added: the four `CONFLUX_S3_*` store credentials (commented, with
  a note that the two key values belong in `.env` and nowhere else),
  `CONFLUX_MIN_REPUTATION_SCORE`, `CONFLUX_REPUTATION_FILTER_ENABLED`,
  and `conflux-node`'s `CONFLUX_SEED_VALUE`. Parity is now exact in both
  directions — 43 read, 43 documented, nothing stale.

  **`docs/USAGE.md` had the same drift, plus two stale claims.** It
  documented 36 of the 43 and now documents all of them
  (`CONFLUX_CONNECTION_MODE`, `CONFLUX_CLIENT_SIDE_PRIVACY_TRANSFORM`,
  and `CONFLUX_SEED_VALUE` into the node table;
  `CONFLUX_REPUTATION_FILTER_ENABLED` into the resolved-parameters
  table; the three `CONFLUX_TEST_*` URLs into "Building and testing").
  The two stale claims were worth more than the count:

  - The listen-address row still said "the admin API has no auth of its
    own" — written before Tier 2 added `CONFLUX_ADMIN_TOKEN`, and
    contradicted by the row two lines above it in the same table.
  - It told readers to run `evnx scan .env .env.example`. **That command
    silently scans nothing for the template**: evnx excludes that
    filename by design, so it prints `Scanned 0 files` above a green
    `✓ No secrets detected` — a false all-clear on the one file that is
    tracked. This is the same defect the CI step hit during Tier 3; the
    doc kept recommending it. Now stated outright, along with why local
    usage (`evnx scan .env`) and CI (`evnx scan .`) correctly differ:
    `.venv` exists here and not in a fresh checkout.

  Three behavioral claims written into these tables were checked against
  the code rather than assumed, and **one of the three was wrong**: the
  first draft said a `pull` node against a push-mode server never
  receives a task. It does — `FlTransportService` implements both RPCs
  unconditionally, so connection mode is the node's own choice. The
  mismatch that *is* real is on the local hop, and that is what the row
  now says. (The other two held: `apply_server_side_privacy` runs
  unconditionally, so enabling the node-side transform really does clip
  twice; and `round.rs`'s filter gates on `reputation_filter_enabled`,
  so `min_reputation_score` alone genuinely does nothing.)

  Verified by running CI's whole `env-files` job locally against a
  `git archive` checkout: 0 high- and 0 medium-confidence findings,
  `.env` untracked, `evnx validate` 0 errors.
- **S8 — CI.** Five jobs: `fmt`, `clippy` (`--all-targets`, warnings
  denied), `test` (with Redis/Postgres/MinIO service containers, so the
  three durable backends are actually exercised), `msrv` (checks the
  1.88 crates on a 1.88 toolchain — 1.85 as originally written, corrected
  2026-09-01), and `env-files` (`evnx scan` plus a
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

## Tier 6 — the audit's own follow-ups (2026-08-31)

Four gaps found by auditing what Tiers 1–5 had *not* looked at. T1 was
the one that mattered: it found four real defects, three of them
remotely triggerable, in code 387 tests already covered.

### T1 — stateful aggregators, tested across rounds

`tests/adversarial_input.rs` builds a fresh aggregator for every
assertion and hands it exactly one batch. Correct for the nine stateless
methods; structurally blind for the four that carry state. The sequence
it cannot express:

1. Round N is hostile but **finite**, so `decode_and_validate` accepts
   it — correctly, since the codec cannot know a model's plausible scale.
2. The aggregator folds it into its stored reference or history.
3. Round N+1 is an ordinary batch from honest clients, and the poisoned
   state turns it into garbage.

Nobody sent a bad update in round N+1. The output is wrong anyway, and
it is the checkpoint. New suite:
[`tests/stateful_adversarial_input.rs`](../crates/conflux-core/tests/stateful_adversarial_input.rs),
9 tests. **All four defects were found by writing it, and each was
confirmed by a failing test before any fix existed.**

- **D1 — `centered_clipping`'s stored reference went `NaN`, permanently.**
  The worst of the four. `u_i − v` overflows `f32` to infinity whenever a
  client sits far from the reference (two finite `f32` weights can be
  `2 · f32::MAX` apart), so `‖u_i − v‖` is infinite, so the clip scale
  `min(1, τ/‖·‖)` is exactly `0.0` — and `inf * 0.0` is `NaN`. The
  clipping step decided *correctly* that this client should move the
  model by nothing, and wrote `NaN` into the running reference doing it.
  Every later round then clips against `NaN`, every aggregate is `NaN`,
  and **no subsequent honest round can recover it**. One finite,
  validation-passing update was enough. Traced to the exact arithmetic
  before fixing, not guessed at.
- **D2 — the seed mean overflowed.** `CenteredClippingAggregator` seeded
  `v` by summing the first batch and dividing by `n` afterwards. Four
  clients near `f32::MAX` overflow the running total to infinity, and
  `inf / n` is still infinity. Identical in shape to the geometric-median
  defect already fixed in `robust.rs`; the pattern had simply been
  written a second time somewhere else.
- **D3 — a client could evade DSS's stability gate by submitting
  *larger* updates.** `l2_distance` overflowed to infinity, making the
  trace mean infinite, `(x − mean)` `NaN`, and `stability` `NaN`. Every
  comparison against `NaN` is false, so `stability < stability_threshold`
  evaluated to "stable" — for the single most erratic client in the
  batch. Precisely backwards, and invisible in any result table, because
  the client simply received full weight and nothing was logged.
- **D4 — `DssAggregator` returned `[inf, inf, inf]` on a single batch.**
  Its own combine path (`combine_through_base = false`) multiplied each
  update by `weight × num_samples` *before* normalizing, and
  `f32::MAX × 10` is already infinity. This one is single-round: the
  existing suite would have caught it years earlier if DSS had ever been
  in it, which is exactly T2.

**Root cause, shared by all four: `f32` intermediate overflow.** Fixed by
computing distances and difference-accumulation in `f64` — which cannot
overflow for any finite `f32` pair, the largest possible squared
difference being ~`4.6e77` against `f64`'s `1.8e308` — and by folding
`1/n` into each term instead of dividing a total that has already
overflowed. Both fixes have precedent in this codebase (the `f64`
collusion score, the geometric-median normalization); neither pattern had
been applied everywhere it belonged.

A sweep for the remaining instances found **three more latent ones** and
fixed them the same way: FABA's iterative mean, Divide-and-Conquer's
centering mean, and FoolsGold's combine. None was reachable by the
existing tests, all were the identical defect.

**One thing the new suite got wrong, and it is worth recording.** Two
assertions initially demanded that `centered_clipping` recover to the
honest consensus after a hostile round. It does not, and it says so: its
own fidelity note (ADR 0008) documents that `v` is seeded from round
one's *plain mean*, so a round-one attacker drags the reference and
clipping then holds it there — "the defense compounds over rounds rather
than arriving fully formed in round one". The test was asserting against
a deliberate, published property. Replaced with
`centered_clipping_movement_stays_bounded_by_the_clip_radius`, which
checks the guarantee the method actually makes: after seeding, no single
round may move the reference by more than `τ`. That is the whole defense,
and it now has a test.

Worth noting how D1 and the documented seed-drag *composed*: the
documented cost (an attacker drags the seed) put the reference far from
every honest client, which is exactly the condition that triggered the
undocumented `NaN`. A known tradeoff and an unknown arithmetic defect met
and produced something worse than either.

### T2 — `DssAggregator` had no adversarial coverage at all

`ALL_AGGREGATORS` iterates `build_aggregator`'s catalog, and DSS is
deliberately not in it (an unvalidated hypothesis — ADR 0008,
`API_STABILITY.md`), so a name-driven suite could not see it. It is also
the aggregator every DSS research run drives. It now faces the same ten
hostile-input scenarios the twelve catalog methods do, on both combine
paths. That test is what found D4.

### T3 — runnable doc examples, 0 → 21

341 public items were documented and `cargo doc` rendered prose with no
code: two ```` ```text ```` fences workspace-wide and **zero** doc-tests.
Every one of the 13 crates now has at least one runnable example, 21
total, all compile-checked by `cargo test`. Doc-tests are the only test
kind that cannot silently drift from the API — they fail to compile when
a signature moves — which matters most for the two projects building on
these crates.

Writing them immediately caught one API error (`evict_stale` for
`evict_expired`) and one real safety property the author had not
expected: `RoundBuffer` refuses a delta whose round does not match, so a
slow client resubmitting last round's work cannot corrupt this round's
batch. That is now an assertion rather than an accident.

### T4 — two docs contradicted the code

`CLAUDE.md` described `docs/E2E_TESTING.md` as planning a harness "not
yet built"; four have existed and run since 2026-08-22. `CLI_DESIGN.md`
gated the CLI on "Tier 5 (production hardening) is still open"; it
shipped. Both corrected.

### Disclosure: the arithmetic changed, and results are not bit-identical

Folding `1/n` into each term rather than dividing afterwards changes
floating-point rounding. Every exact-value unit test passes unchanged,
so no *tested* result moved — but measured directly on 15,378
coordinates at realistic FL magnitudes: **44.8% bit-identical, worst
relative difference `3.4e-7`**, about 2.8× `f32::EPSILON`. Last-bit
territory, four orders of magnitude below the smallest effect
`docs/research/` reports (its conclusions turn on differences like
`16.99 → 0.297`). The 28,331 existing result rows were computed with the
old ordering; a re-run will differ in the last bits and in nothing that
changes a conclusion. Flagged rather than assumed, because the DSS line
reads these numbers.

**Verification.** 387 → **417 tests**, 0 failed, 0 ignored, against real
Redis/Postgres/MinIO. `cargo fmt --check` clean, `cargo clippy
--workspace --all-targets` clean under `-D warnings`.

## F1 + F2 — the two deferred plumbing ADRs, built (2026-08-31)

Both were "proposed — pending project-owner review". Approved and
implemented. They turned out to need each other, which neither
anticipated.

### F1 — ADR 0012: cross-round state and per-client extra fields

Unblocks FedNova, SCAFFOLD and FedOpt, which share this plumbing. **No
aggregator reads either new field yet** — each method is still its own
phase brief, exactly as the ADR says. What changed is that none of them
is blocked on a schema decision any more.

- **Two optional proto fields**, `local_steps` (FedNova) and
  `control_variate` (SCAFFOLD), on `ClientDelta` *and* `DeltaChunk`.
- **The `Mutex` state pattern is now documented rather than incidental**
  — on `Aggregator::aggregate` and in `EXTENDING.md`, with the two
  obligations Tier 6 learned the hard way attached to it.

Three corrections to the ADR, all found by building it and recorded in
the ADR itself:

1. **`ClientDelta` never travels.** The ADR's snippet extends it alone,
   but it is what the server *builds* from a chunk stream — fields added
   only there could never be populated by any client. Both messages now
   carry them, and they reassemble differently: `local_steps` is a scalar
   read from the first chunk to *arrive* (as `num_samples` always has),
   `control_variate` is chunked like `data` and concatenated in
   `chunk_index` order. Different rules, so every reassembly test submits
   out of order — one that did not would pass with them confused.
2. **Tier 5's `max_update_bytes` bound had to learn about the new
   field.** It counted `data` only, because `data` was the only
   client-controlled payload when it was written. Left alone, the H1 fix
   would still have *existed* and been trivially bypassable: put the
   flood in `control_variate`, keep `data` tiny. Confirmed by neutralising
   the fix and watching the new test go red.
3. **"No producer needs to change" is true of bytes, not of Rust.** On
   the wire, yes — and now proven at the byte level against a hand-built
   expected encoding rather than against the type under test. In Rust it
   broke 75 struct literals across 37 files, which now end in
   `..Default::default()` so the next field breaks none of them.

### F2 — ADR 0011: the trusted-reference sidecar

Accepted as **option 2**. FLTrust ships; the server gained zero new
dependencies, exactly as the ADR promised.

| Piece | Where |
|---|---|
| Hop schema | `conflux-proto/proto/trusted_reference.proto` (its own file — a separate contract, not part of the client-facing surface) |
| The client the server uses | `conflux-net::TrustedReferenceTransport` |
| The sidecar | `crates/conflux-trusted-reference` — the fourteenth crate, lib + binary |
| FLTrust | `conflux-core::FlTrustAggregator`, a new `trusted` family |

**This is the first method in the catalog that can resist a colluding
majority.** Every other family derives "normal" from the batch, so a
majority is normal by construction — this document's own research line
measured that (§5.1). FLTrust never asks the batch. Asserted directly:
`a_colluding_majority_does_not_win` puts three attackers against one
honest client and the honest direction still wins.

Three things the ADR left open, decided while building:

- **How an async reference reaches a synchronous `aggregate`.** Via
  **F1's pattern**, which had landed hours earlier: a new
  `set_trusted_reference` trait method with a default no-op, so the
  twelve existing methods were untouched. The two ADRs needed each other.
- **What happens when the reference is missing.** The obvious fallback is
  an unweighted mean — which *is* FedAvg, the method FLTrust replaces,
  substituted at exactly the moment the defense should engage, producing
  a checkpoint indistinguishable from a healthy one. It errors instead,
  and the server refuses to start.
- **Whether the boundary is enforced or merely stated.** ADR 0010 has
  asserted the same kind of invariant about `conflux-attacks` since Phase
  12 — in prose only. CI now has an `isolation` job that fails if
  `cargo tree -p conflux-server -e normal` contains either crate.

**What deliberately did not land.** Zeno: the `ScoreUpdates` RPC exists,
the sidecar serves it, a test drives it over the real hop — but no
aggregator consumes it. And no deep-learning runtime: the shipped
`TrustedModel` is `LinearLeastSquares`, real gradient descent tested to
recover known coefficients from a wrong start, and honestly a *linear*
model. Anything else implements the trait against `ort`/`tch`/a Python
process. That extension point is the whole reason the capability lives
outside the server.

**Verification.** 430 → **451 tests** (F1: 13, F2: 8), 0 failed, 0
ignored. Clippy clean under `-D warnings`, fmt clean. Beyond the suite,
the real binaries were run together: the sidecar loads a CSV root dataset
and serves, the server completes the `Describe` handshake and fetches a
reference in round 1. All three startup paths were checked by hand —
`fltrust` with no sidecar refuses to start, `fltrust` against a dead port
refuses to start, and `fedavg` with a sidecar configured warns and opens
no connection. `.env.example` and `USAGE.md` are both back at exact
parity with the code (48 variables).

**One thing not verified locally.** `conflux-trusted-reference` declares
the workspace MSRV of 1.85 and is now in CI's `msrv` job, but there is no
1.85 toolchain on this machine, so that job will be its first real check.
The evidence that exists is `clippy::incompatible_msrv` running clean
against the declared version — which is what caught `is_multiple_of` in
Tier 3, so it is not nothing, but it is not a 1.85 build either.

> **Resolved 2026-09-01, and the answer was no.** A 1.85 toolchain was
> finally installed: `conflux-trusted-reference` does not build there,
> and neither do seven other crates that made the same promise. The
> workspace MSRV is now **1.88**. See phase 23's entry below. The caveat
> in this paragraph was the right one to write down; it just understated
> how much was riding on it.

## Phase 22 + Experiment 2.10 — the optimization family, and FLANDERS (2026-09-01)

Prompted by a comparison against Flower, Xaynet and OpenFL. Two of those
three are dead — Xaynet archived 2022, OpenFL "no longer under active
development", pointing users at Flower — so the comparison is really
against Flower, and it found one large gap and one urgent research
problem.

### The gap: `optimization`, the framework's largest catalog hole

Conflux shipped ten robust methods against Flower's five built-in, and
**zero** optimization methods against its six. Adaptive server
optimization is the axis that makes federated training converge on
non-IID data, and it was entirely absent.

**FedAdagrad, FedAdam and FedYogi now ship** — Reddi et al. (2021)
Algorithm 2, one type with a discriminant since the three differ in
exactly one line. New config keys `server_learning_rate` (`η`) and
`server_tau` (`τ`), both with full ADR 0007 provenance logging. `τ =
1e-3` is the paper's value; **`η` has no honest default** and its `1.0`
is documented as a placeholder in the same sense `clip_radius = 1.0` is,
because the paper deliberately publishes no universal value.

[`docs/phases/phase-22-optimization-family.md`](phases/phase-22-optimization-family.md)
scopes the four that remain. The short version: **FedAvgM and QFedAvg
are buildable now** (QFedAvg needs one more optional proto field, which
ADR 0012's recipe already covers); **FedProx and SCAFFOLD are gated on
ADR 0005**, because FedProx's server side *is* FedAvg and its whole
substance is a client-side loss term. FedNova sits between — its
server-side half is small, but it needs a client that populates
`local_steps`.

One test failure worth recording: the first version of
`yogis_second_moment_decays_more_slowly_than_adams` asserted the
opposite, that Yogi would recover *faster* after a shock. The
implementation was right and the assertion was backwards — Yogi's `v`
moves additively and therefore decays more slowly than Adam's
multiplicative rule, which is the entire point of Yogi.

### The urgent one: DSS had never been positioned against its closest prior art

**FLANDERS** (Gabrielli, Belli, Matrullo, Miori & Tolomei, 2024, arXiv
2303.16668) is a cross-round pre-aggregation filter that wraps an
arbitrary base and targets >50% malicious under non-IID. That is DSS's
shape, DSS's Claim 1 and DSS's Claim 2. It was cited **nowhere** in
`docs/research/` or in `dss.tex`, and it is reproduced as a Flower
baseline, so it is the first thing a reviewer would reach for.

Implemented faithfully in `conflux-core` (`flanders.rs` — MAR(1) fitted
by alternating least squares each round, `δ = ‖·‖²₂`, top-`k`, the
paper's cold-start branch) and compared head to head: **10,800 rows, 5
seeds, 6 attacks, malicious ratios from 20% to 80%**
([§5.14](research/temporal-consistency-aggregation.md)).

Four findings:

1. **On the attack DSS was validated against, DSS wins by ~15×** —
   0.64 ± 0.34 vs 9.41 ± 7.46, against undefended FedAvg's 553.0.
   Non-overlapping intervals.
2. **In FLANDERS's own headline regime, it fails and DSS holds.** At 60%
   malicious it scores 1901.7 where undefended FedAvg scores 1659.1 —
   worse than no defense; at 80% it collapses entirely. DSS holds at
   0.44 and 7.19.
3. **FLANDERS is worse than plain FedAvg against every Sybil attack
   tested** (24.2 vs 17.0 at 20% malicious), and the mechanism is
   structural rather than a defect: a colluder that repeats itself is
   the *most forecastable* client in the batch, so a forecast-
   consistency filter keeps it and drops the noisier honest majority.
   Pinned as a unit test. Its own paper's attacks all perturb or
   optimize, so this failure mode cannot arise there.
4. **Collusion-only DSS dominates everything**, including the shipped
   AND-gate, by one to two orders of magnitude across all six attacks —
   the third independent finding pointing at the gate after §5.6 and
   §5.12. Still does not license flipping it: none of these attacks
   includes a legitimately-noisy honest majority, which is exactly what
   the stability conjunct protects.

**DSS's contribution 3 was withdrawn.** "Composability with any existing
`Aggregator`" is FLANDERS's shape, published first. Withdrawn in both
the research doc and `dss.tex` rather than defended. What survives is
signal dimensionality (a `d × h` matrix forecast against a length-`w`
scalar trace) and the measured finding that the two methods fail on
*opposite* attack shapes.

`dss.tex` gained a related-work paragraph, a results section
(`sec:flanders`), two bibliography entries, and the withdrawal. Still
not compiled — no TeX toolchain here — but checked structurally: braces
balanced, all 17 `tabular` environments closed, every `\cite` resolving
to a `\bibitem` and every `\ref` to a `\label`.

**Fidelity correction found by measuring.** The catalog first paired
`flanders` with FedAvg. The paper specifies `ϕ = Krum or any other
existing robust aggregation heuristic`, and finding 3 shows the FedAvg
pairing is actively harmful, so the catalog entry now pairs it with Krum
as written. Shipping the harmful pairing would have misrepresented the
method.

**Verification.** 451 → **469 tests**, 0 failed, 0 ignored. Clippy clean
under `-D warnings`, fmt clean. The 10,800-row results file was
re-generated after a later refactor of the linear solver and came back
**byte-identical**, which is the reproducibility rule
`docs/research/AGENT.md` requires. `.env.example` and `USAGE.md` are both
at exact 50/50 parity with the code.

## Defect: FLANDERS was unrunnable on a real model (2026-09-01)

Found by running it, in the worst way: `conflux-server` was OOM-killed
twice at **8.3 GB and 6.6 GB resident**, and because the box has 14 GB
with swap already full, the kernel's global OOM killer took the desktop
session with it.

**Cause.** `FlandersAggregator`'s MAR coefficient matrix is `d × d`. At
MNIST's 50,890 parameters that is `50890² × 8 bytes` = **20.7 GB for a
single allocation**, and `fit_mar` builds several. It first fires in
round *three* — the first round with enough history to fit anything —
which is why a two-round smoke test passed and a six-round sweep died.

**Why nothing caught it.** Every synthetic experiment runs at `dim = 3`,
where the same allocation is 72 bytes. The entire test suite, the
adversarial suites included, ran at a dimension four orders of magnitude
below where the defect lives.

**The fix is the paper's own.** FLANDERS samples 500 coordinates "for
tractability on real models"; `max_forecast_dim` now does the same
(evenly spaced, deterministic — a contiguous prefix would forecast one
layer of a real network and ignore every other). Peak RSS at
`dim = 50,890` went from >8 GB to **258 MB**, and there is now a
regression test at that dimension.

**The comment that predicted it.** The implementation's own fidelity
note said: *"The paper samples 500 coordinates for tractability on real
models. This implementation uses all of them... A deployment on a large
model would want the subsampling; it is not implemented rather than
being approximated silently."* The hazard was documented, correctly, and
then walked into. The reasoning was the error: subsampling is not an
approximation *of* the paper, it **is** the paper, and omitting it left
the method unable to run at the scale it was written for.

**A second, smaller leak fixed alongside.** All four e2e demo harnesses
kept their `mktemp -d` work directory unconditionally — "kept for
inspection". On most Linux systems `/tmp` is tmpfs, so that is *RAM*:
23 abandoned directories at ~22 MB each had accumulated. They now clean
up on success and are kept only on failure, or with
`CONFLUX_KEEP_WORK_DIR=1`.

**Verification.** 469 → **471 tests**, 0 failed. Experiment 2.10 re-runs
**byte-identical** (dim = 3 is below the bound, so no published result
moved). Peak RSS for the full workspace test run: 1.3 GB.

## conflux-web synced to 0.2.0, and FedAvgM (2026-09-01)

### The documentation site had drifted badly

Measured rather than estimated: **5 of the then-17 aggregation methods
missing** (`fltrust`, `flanders`, `fedadagrad`, `fedadam`, `fedyogi`),
**5 of 26 config parameters missing**, and **zero files** mentioning the
trusted-reference sidecar, the optimization family, ADR 0012's proto
fields, `max_update_bytes`, admin-token auth, graceful shutdown, or the
`/health` breaking change. Nine sessions of framework work had landed
with none of it reaching the site.

Now at parity. What changed:

- **`reference/aggregation-catalog`** restructured from one flat table
  into the five families, with all 19 methods and the two measured
  cautions (`flanders` must be paired with a robust base; `clip_radius`
  and `server_learning_rate` are placeholders, not recommendations).
- **`reference/configuration-catalog`** — all 26 resolved parameters.
- **`reference/crates`**, `guides/architecture`, and a blog post — 13 →
  14 crates.
- **New `crate-deep-dives/conflux-trusted-reference`** — the sidecar as a
  boundary drawn as a process, and why the server can call one without
  linking one.
- **`guides/deployment`** — the `/health` JSON shape as a flagged
  breaking change, why `degraded` returns 200 and `stopped` 503,
  SIGTERM draining, `max_update_bytes`, and four new checklist items.
- **`guides/extending`** — ADR 0012's `Mutex` state pattern with the
  three obligations that come with statefulness, the add-a-proto-field
  recipe, and ADR 0011's trusted-family hooks.
- **`conflux-proto`/`conflux-net`/`conflux-core`/`conflux-server` deep
  dives** — optional-field wire compatibility, the sidecar client, the
  byte bound and why every payload field must count toward it, the
  round-loop health fix.
- **`getting-started`** — the new `/health` output and the admin-token
  requirement.

42 pages, builds clean. Two errors I introduced and caught while
verifying: a blog line where I changed "Two of them" (the count of
zero-dependency anchor crates) along with the crate count, which made it
disagree with the two crates it then named; and a claim that "the three
families below each answer that ceiling", when `optimization` is
explicitly orthogonal to it.

### FedAvgM shipped — the `optimization` family is now four

Hsu, Qi & Brown (2019). `v ← βv + Δw`, `w ← w − v`, and nothing else.
The baseline every adaptive method is measured against, including Reddi
et al.'s own results table — a framework with FedOpt and without this
cannot reproduce it. (Took the catalog to 18; q-FedAvg below makes 19.)

Built as its own type rather than a fourth `FedOptVariant`, because that
enum's honesty rests on its three arms differing in exactly one line of
Algorithm 2 and FedAvgM has no second moment at all.

**Two paper-level disagreements, both implemented as written rather than
harmonized:**

- **The two papers weight differently.** FedAvgM's `Δw` is
  `Σ (n_k/n) Δw_k` — sample-count weighted. FedOpt's Algorithm 2 line 10
  is an *unweighted* mean. A test
  (`fedavgm_weights_by_sample_count_unlike_fedopt`) pins the difference
  so nobody later "fixes" one to match the other.
- **The FedAvgM paper disagrees with itself.** §4.2's equation is
  classical momentum; its experiments say Nesterov. The equation is what
  ships, and the discrepancy is documented rather than silently
  resolved.

`β` is config-reachable as `server_momentum` (builtin 0.9 — a *real*
default, inside the paper's own sweep, unlike `server_learning_rate`'s
placeholder). `η` is fixed at 1.0 by that paper, so it is the one
honest default in this family.

### q-FedAvg shipped — and the ADR 0012 recipe proved itself

Li, Sanjabi, Beirami & Smith (2020), Algorithm 2. The **only
fairness-oriented method in the catalog**: it weights each client by its
own loss raised to `q`, flattening the accuracy *distribution* rather
than only improving its mean. `q = 0` is exactly FedAvg, which is the
builtin — selecting the method without choosing a `q` should behave like
the thing it generalizes, not silently apply a trade.

It needed a third optional wire field (`local_loss`), which is the first
independent exercise of ADR 0012's add-a-field recipe. **It broke
exactly one struct literal in the whole workspace** — the compatibility
test that deliberately names every field because noticing schema growth
is its job. The `..Default::default()` idiom introduced when the first
two fields landed did what it was for, and byte-level backward
compatibility still holds with three optional fields present. Only two
of the recipe's three edits applied: a fixed-size scalar needs no
`max_update_bytes` accounting.

**Two of my test premises were wrong here, and the implementation was
right both times.** I assumed the step magnitude would rise
monotonically with `q`, then that it would fall monotonically. It does
neither — 0.538 at `q=1`, 0.624 at `q=2`, 0.499 at `q=4` — because two
effects compete: `F^q` weighting turns the update further toward the
worst-served client and *lengthens* the step, while `h_k`'s
`q·F^{q−1}·‖Δw_k‖²` term grows with `q` and *shortens* it. So **`q` is
not a simple "more fairness" dial**: it trades mean accuracy,
uniformity, and convergence speed simultaneously, non-monotonically.
Both facts are pinned as tests now rather than left as intuitions.

Unusable until a client reports `local_loss` — with none reported it
falls back to FedAvg, which is honest but is not q-FedAvg. Like FedNova
and SCAFFOLD, its remaining blocker is ADR 0005.

**Four methods now wait on that one decision** (FedProx, SCAFFOLD,
FedNova, q-FedAvg's real use). Everything buildable *without* a client
change is built. That is the strongest argument yet for settling the
Python SDK question.

**Verification.** 471 → **482 tests**, 0 failed. Clippy clean under
`-D warnings`, fmt clean. **19 methods across five families.**
`.env.example` and `USAGE.md` both back at exact **53/53** parity.

## Experiment 3.3 — the FLANDERS comparison on a real model (2026-09-01)

Task `r8`, and the reason it mattered: §5.14's headline claims were
entirely at `dim = 3`, and §5.13 is the standing evidence that a
synthetic conclusion need not survive a real one.

**Getting there is the defect recorded above** — FLANDERS's `d × d`
matrix, 20.7 GB at 50,890 parameters, two OOM kills. Fixed with the
paper's own subsampling. The consequence for the research line is
sharper than the caveat §5.14 wrote: **FLANDERS's forecast is quadratic
in model dimension and DSS's scalar traces are constant in it.** That is
a difference of kind, and it was nearly missed.

**The accuracy result reproduces §5.14's Finding 3 and strengthens it.**
Real MNIST, 50,890-parameter MLP, 3 clients, Dirichlet `α = 0.5`,
baseline 0.852:

| aggregator | no attack | poisoned |
|---|---|---|
| `fedavg` | **0.839** | 0.181 |
| `krum` | 0.669 | **0.655** |
| `flanders` (= FLANDERS + Krum) | 0.671 | **0.102** |

`flanders` *is* `krum` plus a pre-filter. Undefended they are
indistinguishable. Under attack the filter takes its base from 0.655 to
**0.102 — below undefended FedAvg**. Synthetically only the FedAvg
pairing was harmful; on a real model the paper's own Krum pairing is.

**A structural limit, worth deciding rather than leaving implied.** DSS
cannot be measured on real data *at all*: the harness drives the
production `conflux-server` binary, which builds aggregators from
`build_aggregator`'s catalog, and `AGENT.md`'s own rule keeps
`DssAggregator` out of it. The rule that protects users from an
unvalidated method is the same one preventing it being validated. Every
real-data result in `docs/research/` is therefore about catalog methods
only.

**The harness is no longer fixed-seed**, and the headline comparison is
no longer `n = 1`. `run_demo.sh` gained `CONFLUX_DEMO_SEED` (defaulting
to the old 42, so existing results reproduce) — the blocker behind task
`r4`. Repeating `krum` vs `flanders` across three partitions (§5.16.1):

| aggregator | mean ± 95% CI |
|---|---|
| `krum` | **0.689 ± 0.042** |
| `flanders` | **0.114 ± 0.021** |

Non-overlapping, identical direction in every seed, and `flanders` sits
at chance (0.100 for ten-class MNIST). **Adding FLANDERS's filter in
front of Krum takes a working defense to approximately random.**

**A limitation the experiment found about itself.** `CONFLUX_DEMO_SEED`
seeds the data partition but *not* the trainers — noticed because the
same nominal seed 42 gave `krum` 0.655 in §5.16 and 0.718 in §5.16.1.
That 0.063 spread is not noise to explain away; it is a free measurement
of the harness's residual variance, and the effect being reported is
**9.1× larger than it**. Seeding the trainers is the remaining half of
`r4`.

## Phase 23 — the `ClientApp` SDK, in Python and in Rust (2026-09-01)

ADR 0005 separates three questions the phrase "the SDK" usually
conflates: (1) how a client learns what model to train, (2) how a
participant *gets* the client code, (3) what the SDK wraps. It
recommends resolving **(3) first**, because it is the only one this
codebase can answer alone. That is what shipped, and nothing more.

**`python/conflux_client/app.py`.** A `ClientApp` base class owning
everything previously copy-pasted into every harness: connect, register,
the fetch-until-a-new-round loop, placeholder-init detection, f32-aligned
chunking, submit-with-retry, and treating a round that closed
mid-training as ordinary rather than fatal. **Four separate copies of a
`struct.pack`/`unpack` codec** existed across the harnesses and the stub;
there is now one. The MNIST harness is migrated and verified end to end
(0.142 → 0.858 held-out against a 0.750 centralized baseline, real gRPC,
real training). CIFAR-10, Shakespeare and numpy-logreg still carry their
own copies.

**This is what unblocks four methods.** `TrainResult` carries
`local_steps`, `local_loss` and `control_variate` — the ADR 0012 wire
fields that have existed since 2026-08-31 and that **nothing has ever
been able to populate**, which is exactly why FedNova, SCAFFOLD and
q-FedAvg were shipped-but-inert. The migrated MNIST harness is the first
client in the project's history to send any of them.

### Two things found while building it

**The generated Python stubs were stale.** `fl_transport_pb2.py` predated
ADR 0012 entirely — no `local_steps`, no `control_variate`, no
`local_loss`. The Python side had been silently out of sync with the
schema since those fields landed, and nothing would have noticed:
generated files are not committed and no test imports them. Regenerating
is one command; *knowing to* was the problem. Still worth a CI step.

**`absent` and `zero` are the same value in Python.** protobuf reads an
unset `optional float` as `0.0`, so a client checking truthiness cannot
distinguish "not running q-FedAvg" from "reported a loss of exactly
zero" — and under `q > 0` a loss read as zero means *zero weight*,
silently excluding every client not yet upgraded. `HasField` is the only
correct check. Pinned as tests on both sides of the wire. This one cost
a wrong test premise before it was understood.

### The Rust client, built rather than argued about

`crates/conflux-client` — the fifteenth crate, and the spike the phase
brief proposed. Same contract as the Python SDK, field for field, so a
divergence between them is a bug in one rather than a design choice.

**It needed no new proto field, no server change, and no `conflux-node`
change.** `PullTransport` already *was* the client half of the local hop,
because ADR 0004 made both hops speak one schema — so the Rust client
reuses the transport the Python client talks to rather than paralleling
it. That is the architectural result: the loopback hop is a language
boundary, not a design seam, and removing it removes a *process*, not a
layer.

Measured on a real federation — real `conflux-server`, four real
`conflux-node`s, four Rust clients, eight rounds, no Python process
anywhere:

```
rc-0 (sees feature 0): local-only 0.682 -> federated 0.996
rc-1 (sees feature 1): local-only 0.666 -> federated 0.996
rc-2 (sees feature 2): local-only 0.682 -> federated 0.996
rc-3 (sees feature 3): local-only 0.676 -> federated 0.996
```

Round 2 scored 0.986, round 3 0.994, rounds 4–8 0.996.

**The `local-only` column is what makes this evidence.** The first
version of the example sharded IID and every client reached 1.000 on its
own data *before* federating — proving the loop ran and nothing else. It
was rebuilt so client *i* sees data where only feature *i* varies against
a label of `sum(x) > 0`, so **no client can solve the problem alone**,
and all four are scored on one shared held-out global test set.

**What it does not decide: which ML framework.** Logistic regression
needs none, which is why it is the right spike — it isolates the
*architecture* from the *ML stack*. Hidden layers want Burn, which is
pre-1.0 and says so, and that is a separate evaluation with a separate
cost. And it does not replace Python: researchers want PyTorch, all four
e2e harnesses are PyTorch, and the DSS line runs on them. It is a
*second* path, permanently.

### The MSRV was wrong, and had been for some time

Found while adding `conflux-client` to CI's `msrv` job. Tier 3 recorded
`rust-version = 1.85` as "measured rather than guessed", and the F1/F2
entry above flagged honestly that no 1.85 toolchain existed on this
machine so CI would be its first real check. Installing one gave the
answer: **eight of the twelve crates making the 1.85 promise cannot
resolve at 1.85.** `tonic` 0.14, `jsonwebtoken` 11 and `time` 0.3.55 each
declare 1.88, and everything above `conflux-proto` pulls at least one.
Only `conflux-config`, `conflux-selector`, `conflux-privacy` and
`conflux-reputation` genuinely built there.

`Cargo.lock` is committed, so CI would have resolved identically and the
`msrv` job would have failed on its first run — the claim was false in
the published metadata, not merely untested.

**Why `clippy::incompatible_msrv` did not catch it**, despite catching
`is_multiple_of` in Tier 3: it checks whether the *std APIs we call*
exist at the declared version. It says nothing about whether our
dependencies will build there. Only a real toolchain answers that, which
is the general lesson — a lint that validates one half of a claim reads
like it validates the whole one.

Corrected to **1.88** in `[workspace.package]`, with the two
`aws-sdk-s3` crates still overriding to 1.94.1. Verified by building all
twelve on a real 1.88.0 toolchain, and `conflux-client` is now in the
job.

**The correction round-tripped Tier 3's own workaround.** Raising the
floor past 1.87 made `usize::is_multiple_of` available again, so
`clippy::manual_is_multiple_of` immediately fired on the `% 4 != 0` that
Tier 3 had written *to avoid it* — in `conflux-proto`, and in two other
crates that had copied the shape. Under `-D warnings` that is a build
failure, so the toolchain refused to let the workaround outlive its
reason. Three sites back to `is_multiple_of`.

### Verification

490 tests (467 unit/integration + 23 doc), fmt clean, clippy clean under
`-D warnings`. One earlier run showed `sigterm_exits_cleanly_and_promptly`
failing; it passed in isolation twice and in the clean full run, so it is
a timing-sensitive test losing a race under machine load — worth knowing
before it is read as a regression.

**What remains deferred, deliberately:** ADR 0005's questions (1) model
handoff and (2) code distribution. (2) is a product decision, but no
longer one being made on incomplete information — "one static binary"
for `crowdsource`/`edge` is now a demonstrated option rather than a
claim.

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

**Stabilization: Tiers 1–6 are complete — `conflux-fl` is stable.**
What's left on the framework line, in the order it makes sense to take
it. Note that every remaining item *adds* public API — ADR 0012 changes
`conflux-proto`, the layer `API_STABILITY.md` calls the most stable of
all — so taking them makes the framework more capable and less settled
at the same time. That is a reason to sequence them deliberately, not a
reason to avoid them.

- ~~ADR 0012's stateful-aggregator proto extension~~ and ~~ADR 0011's
  trusted-reference sidecar~~ — **both built (2026-08-31)**.
- ~~FedOpt~~, ~~FedAvgM~~, ~~q-FedAvg~~ — **built (2026-09-01)**, see
  [phase 22](phases/phase-22-optimization-family.md). The `optimization`
  family went from empty to five members.

**Everything buildable without a client-side change is now built.** The
remaining method work all converges on one decision:

| Blocked on | What it needs from a client |
|---|---|
| **FedNova** | populate `local_steps` (already on the wire) |
| **SCAFFOLD** | maintain and send a control variate (already on the wire) |
| **FedProx** | add a proximal term to its own loss — its server side *is* FedAvg, so there is nothing to build here at all |
| **q-FedAvg** *(shipped, inert)* | report `local_loss` (already on the wire); with none reported it falls back to FedAvg |

~~**Four methods, one blocker: ADR 0005's Python SDK question.**~~
**Unblocked 2026-09-01** by phase 23's `ClientApp` SDK — in Python
(`python/conflux_client/app.py`) and in Rust (`crates/conflux-client`).
Both carry `local_steps`, `local_loss` and `control_variate`, so all four
are now ordinary build tasks rather than a pending decision:

- **q-FedAvg** stops being inert the moment a client reports a
  `local_loss` — no server work at all.
- **FedProx** is entirely client-side; its server half *is* FedAvg.
- **FedNova** and **SCAFFOLD** need their server-side aggregators
  written, against fields a client can now actually send.

The decision that remains from ADR 0005 is (2), client code
distribution — which blocks no method.

**Zeno** is the exception — it needs no client change. The sidecar
already serves its scoring RPC and a test drives it over the real hop;
nothing consumes it. Its combine (score, then keep a top-scoring subset)
is its own brief. Note that §5.16's measurement of FLANDERS is a caution
here: a filter that ranks clients can make a robust base *worse*, and
Zeno is a filter that ranks clients.

**Other feature gaps**: Phase 21's profile-file `inherits` (the last
open half of spec §11 Open Item 2) · a `cflux` CLI (designed, deliberately
sequenced last) · `PerClient` epsilon accounting (ADR 0006, gated on
per-client round history).
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

`conflux-web` is **synced as of 2026-09-01** — all 19 methods, all 26
config parameters, the sidecar, the optimization family, and the
`/health` breaking change. It had drifted through nine sessions of
framework work before that, so it is worth re-checking whenever a
catalog entry or config key is added, not batched again.

The DSS research line is below.

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
  it's the only one this codebase can answer alone. **The interface
  question is now shipped** — see
  [phase 23](phases/phase-23-client-app-sdk.md). (1) and (2) remain
  deferred on purpose.
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
