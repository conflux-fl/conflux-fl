# API stability

What `conflux-*` promises about its public interfaces, and what it
doesn't. Written because two other projects — the documentation site and
the DSS research line — now build on these crates, and "whatever compiles
today" is not something either can plan against.

## The promise, in one line

**At `0.x`, breaking changes land in minor versions and are listed in
`STATUS.md`.** That is a real commitment to *disclosure*, not to
stability. Nothing here is `1.0`, and the `0.` is load-bearing.

## Why not 1.0 yet

Three things would have to be true first, and none is:

1. **The public surface has not settled.** The `0.2.0` release alone
   added `AggregatorParams`, `ConnectionMode`, `AdminToken`,
   `JwtKeyMaterial`, and `DssAggregator::combine_through_base`, and
   changed `build_aggregator`'s signature. Some of those were responses
   to defects found by measurement; more measurement is planned.
2. **Two of the crates are research instruments, not products.**
   `conflux-attacks` exists to break things and is `publish = false`;
   `conflux-core`'s `temporal` family contains `DssAggregator`, an
   explicitly unvalidated hypothesis. Freezing their interfaces would
   freeze research in progress.
3. **Open design questions touch public types.** The stability/collusion
   combination rule, whether `clip_radius` should have a
   dimension-aware default, and the `PerClient` accounting scope's
   evolution all have public API consequences that haven't been decided.

A `1.0` before those settle would be a promise made in order to look
finished.

## What counts as public

A crate's public API is what `cargo doc` renders. Every crate carries
`#![warn(missing_docs)]`, so anything public is documented — an
undocumented public item is a build warning, not an oversight to
discover later.

Three things are public but explicitly **not** covered by the promise
above:

| Not covered | Why |
|---|---|
| `conflux-attacks`, entirely | Test/dev only (ADR 0010). `publish = false`. It exists to be changed as new attacks are studied. |
| `conflux-core`'s `DssAggregator` and `ClientDssDiagnostic` | An unvalidated research hypothesis, deliberately absent from `build_aggregator`'s catalog. Its shape follows the research, and the research is ongoing. |
| Anything named in a `docs/phases/*.md` brief as provisional | The brief says so, and the brief is the record. |

## Stability by layer

Not every crate is equally settled, and pretending otherwise would be
less useful than saying which is which.

| Layer | Crates | How settled |
|---|---|---|
| **Wire contract** | `conflux-proto` | Most stable. Changing it breaks every deployed client *and* the Python side simultaneously. Fields are added, never repurposed or renumbered. |
| **Family traits** | `Aggregator`, `ClientSelector`, `PrivacyMechanism`, `ContributionScorer` | Stable in shape. These are the extension points ADR 0002 exists to protect: adding a method means a new trait impl, not a trait change. `Aggregator::aggregate`'s `&self` is deliberate and load-bearing — see ADR 0012. |
| **Backends** | `Registry`, `Store`, `NodeAllowlist`, `PrivacyRoundLog` | Stable in shape, growing in surface. `PrivacyRoundLog` gained per-client methods in Phase 14 without breaking existing implementors. |
| **Server internals** | `AppState`, `run_round`, `RoundSummary` | Least settled. `AppState`'s fields are public for testing convenience, and it is where new subsystems land. Depend on it expecting churn. |
| **Constructors and params** | `build_aggregator`, `AggregatorParams`, and friends | Moderately settled. `AggregatorParams` exists *because* the previous positional-argument shape did not survive a second parameter; it is designed so a third is a field rather than a signature break. |

## Conventions a caller can rely on

These hold across every crate and are not expected to change:

- **Every fallible public function returns a `thiserror`-derived enum,
  never a `String`.** Error variants carry the values you would need to
  act on them — which client, which coordinate, which path — rather than
  formatting them into a message and discarding the structure.
- **No public function panics on caller input.** Aggregators may reject a
  batch, but must never panic and must never return a non-finite value;
  `conflux-core/tests/adversarial_input.rs` enforces this against every
  shipped method. For the four aggregators that carry state across rounds,
  the promise extends to that state:
  `conflux-core/tests/stateful_adversarial_input.rs` enforces that no
  sequence of accepted batches can leave an aggregator unable to handle a
  clean one. Tier 6 added it because the single-batch suite could not
  express that failure, and four real defects were living in the gap. Startup functions in the binaries *do* panic on
  misconfiguration, deliberately — failing to start is the correct
  response to a deployment that would be unsafe.
- **Adding a strategy is additive.** A new aggregator, selector, or
  privacy mechanism is a new trait impl plus an `inventory::submit!`.
  No existing signature changes, and `conflux-server` is untouched.
- **Defaults match cited papers** (ADR 0008). A framework-imposed
  behavior that deviates from a method's published definition is opt-in,
  off by default, and documented as a deviation. `clip_radius = 1.0` is
  the sharp edge here: a placeholder the config layer needs, not a
  recommendation — see its own documentation.

## Adding to the public API

Before making something `pub`, the question is whether a caller outside
this workspace has a reason to name it. Two failure modes to avoid:

- **Public for testing.** Use `pub(crate)` plus `#[cfg(test)]`, or a
  test-only accessor documented as such. `DssAggregator::
  last_diagnostics` is the accepted shape: explicitly a read-only
  diagnostic, documented as never consulted by `aggregate` itself.
- **Public because it was easier.** `AppState`'s public fields are the
  standing example of this, and they are why that row above says
  "expect churn". Not a pattern to extend.

## When something breaks

A breaking change gets a `STATUS.md` entry stating what broke, why, and
what to do instead. The bar for making one is that the current shape is
*wrong*, not merely improvable — the `f32` → `f64` collusion score and
the `build_aggregator` signature both cleared it, because one was
producing float noise where a trust judgment belonged and the other could
not accept a second parameter without becoming transposable.
