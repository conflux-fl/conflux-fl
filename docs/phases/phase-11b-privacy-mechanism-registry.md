# Phase 11b — Wiring `privacy_mechanism` into the strategy registry

## Scope

Phase 10b wired `aggregator`/`selector` into `conflux-config`'s
`inventory` strategy registry but deliberately left `privacy_mechanism`
(`GaussianClippingPrivacy`, the local-DP "clip + noise" mechanism) out,
to keep that phase's diff reviewable (`docs/phases/
phase-10b-strategy-registry-wiring.md`'s scope note). This phase closes
that gap — the third and last of the three families spec §5 names
(`averaging`/`robust` aggregation, client `selector`, `dp` privacy) is
now consistently registry-selectable, not two out of three.

`GaussianClippingPrivacy` currently has no trait at all — it's a bare
struct, called directly (`AppState.privacy: GaussianClippingPrivacy`,
`state.privacy.transform(weights, &mut rng)` in `round.rs`) because it's
always been the only member. This phase gives it the same `Aggregator`/
`ClientSelector`-shaped treatment: a trait, a `Box<dyn _>` field, a
`build_*` factory.

## Inputs
- `conflux-privacy::GaussianClippingPrivacy` (Phase 2c) — `clip`/
  `add_noise`/`transform`, currently generic over `rng: &mut impl
  rand::Rng`.
- Phase 10b's exact pattern (`conflux-core::build_aggregator`,
  `conflux-selector::build_selector`) — this phase repeats it for a
  third crate rather than inventing a new shape.

## Deliverables
- `conflux-privacy::PrivacyMechanism` trait: `fn transform(&self,
  weights: &mut [f32], rng: &mut dyn rand::Rng)`. The existing
  `add_noise`/`transform` inherent methods change from `rng: &mut impl
  rand::Rng` to `rng: &mut dyn rand::Rng` — required for object safety
  (a trait object can't have a generic method), and a no-op change at
  every call site: passing a concrete `&mut StdRng`/`&mut ThreadRng`
  where `&mut dyn Rng` is expected is an automatic, zero-cost unsized
  coercion, so no caller's code changes.
- `conflux-privacy` gains `conflux-config` + `inventory` dependencies
  (matching `conflux-core`/`conflux-selector`'s Phase 10b precedent);
  `inventory::submit! { StrategyEntry { kind: StrategyKind::
  PrivacyMechanism, name: "gaussian_clipping" } }`;
  `build_privacy_mechanism(name: &str, clip_norm: f32, noise_multiplier:
  f32) -> Result<Box<dyn PrivacyMechanism>, PrivacyMechanismBuildError>`
  — takes the two numeric parameters explicitly (unlike `build_aggregator`/
  `build_selector`, `GaussianClippingPrivacy` has no useful
  zero-argument default; `clip_norm`/`noise_multiplier` are already
  resolved config values by the time this is called).
- `conflux-server::app_state.rs`: `privacy: GaussianClippingPrivacy`
  field becomes `privacy: Box<dyn PrivacyMechanism>`; `assemble` calls
  `conflux_privacy::build_privacy_mechanism(&config.privacy_mechanism.value,
  config.clip_norm.value, config.noise_multiplier.value)`, `.expect()`-ing
  on an unknown name — same "startup-invariant, not a runtime `Result`"
  treatment Phase 10b established, `AppState::new`'s signature
  unaffected.

## Test plan
- `build_privacy_mechanism` succeeds for `"gaussian_clipping"`, fails for
  an unknown name, and a registry-sync test (Phase 10b's pattern) checks
  `inventory::submit!` and the match arm stay in sync.
- Every pre-existing `conflux-privacy` test (clip/noise/transform)
  passes completely unmodified — the `impl Rng` → `dyn Rng` signature
  change is invisible at existing call sites.
- `conflux-server`: an explicit `Overrides { privacy_mechanism:
  Some("gaussian_clipping".into()), .. }` resolves through the registry
  and a real round's submitted deltas are visibly clipped/noised as
  before (reuses an existing privacy-affecting assertion pattern from
  Phase 5's own tests, applied to the registry-constructed instance
  instead of the hardcoded one).

## Definition of done
- [x] `cargo test -p conflux-privacy -p conflux-server` passes.
- [x] `cargo build --workspace` and `cargo clippy --workspace --all-targets`
      stay clean.
- [x] Every pre-Phase-11b test still passes unmodified.
- [x] `docs/STATUS.md` updated — all three spec §5 families now
      registry-wired, not two of three.

## Outcome

Implemented exactly as specced. `PrivacyMechanism` trait added;
`GaussianClippingPrivacy`'s inherent `add_noise`/`transform` changed from
`rng: &mut impl rand::Rng` to `rng: &mut dyn rand::Rng` (required for the
trait to be object-safe) — confirmed a true no-op at every existing call
site (all 7 pre-existing `conflux-privacy` tests and
`conflux-server::round.rs`'s own call both passed unmodified, via
automatic unsized coercion). `build_privacy_mechanism` + one
`inventory::submit!` for `"gaussian_clipping"`. `AppState.privacy` is now
`Box<dyn PrivacyMechanism>`, constructed in `assemble` the same
"infallible + `.expect()` on unknown name" way `aggregator`/`selector`
already are.

Real tests: `conflux-privacy`'s own registry-sync and build-success/
failure tests, plus one confirming the registry-constructed instance
clips exactly like the concrete type did before this phase.
`crates/conflux-server/tests/privacy_mechanism_registry.rs`: an explicit
`privacy_mechanism` override resolves through the registry and a real
`GaussianClippingPrivacy` instance still clips correctly; an unknown name
panics at construction.

181 tests passing workspace-wide (was 175 at the end of Phase 11a),
stable; `cargo fmt --check` and `cargo clippy --workspace --all-targets`
both clean. All three spec §5 families (`averaging`/`robust` aggregation,
`selector`, `dp` privacy) are now registry-wired.
