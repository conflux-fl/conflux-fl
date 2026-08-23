# 0010 — A dedicated crate for known FL attacks, dev/test-only

## Context
Phase 11a shipped real Byzantine-resilient aggregation (Krum, Multi-Krum,
Trimmed Mean, Median). Its own tests prove each method resists *ad hoc*
adversarial inputs (a large-magnitude outlier, a sign-flipped update) —
useful, but not the same claim as "resists the attacks the FL robustness
literature actually studies." Krum and Trimmed Mean/Median were
specifically challenged, and in some regimes broken, by attacks designed
*after* they were published — most notably Baruch, Baruch & Goldberg
(2019)'s "A Little Is Enough" (ALIE), which stays within the statistical
bounds honest updates would naturally have rather than presenting as an
obvious outlier. Testing only crude outliers would materially overstate
what these defenses actually guarantee.

## Decision
A new crate, `conflux-attacks`, outside the spec v1 twelve-crate layout
(spec §2) — implements cited, published FL attacks as `Attack: fn
craft(&self, honest_updates, num_attackers) -> Vec<ClientDelta>`
implementations, mirroring the defense side's citation discipline (ADR
0008) exactly, just for adversaries instead of aggregators.

**Test/dev-only, never a `conflux-server` dependency.** This is
attack-simulation code for validating defenses, not a runtime component
— `conflux-server`'s production dependency graph is unaffected. It
depends on `conflux-proto` (for `ClientDelta`) and, as a **dev-only**
dependency for its own application-level tests, `conflux-core` (to run
real `Aggregator`s against crafted attacks) — never the reverse.

**Application-level, not just unit-level, tests.** `conflux-attacks`'s
own test suite runs each attack against each shipped `Aggregator`
end-to-end (the actual `aggregate()` call, not a mocked one) and reports
whether the honest consensus survives — an attack/defense matrix, not
just "the crafted vector has the expected shape." Where a defense
doesn't hold (e.g. ALIE against certain parameter regimes, matching the
literature's own finding), the test says so honestly rather than being
tuned to always pass — see `docs/phases/phase-12-attack-simulation.md`.

## Consequences
- A future defense claim (a new `robust` family member, a tightened
  `robust_byzantine_fraction` default) should be checked against this
  crate's attack suite before being trusted, not just its own poison
  tests — the two are complementary, not redundant (Phase 11a's poison
  tests prove a defense resists *something* adversarial; this crate's
  tests prove it resists *specific, published, sometimes
  defense-aware* attacks).
- New attacks follow the same citation discipline as new defenses (ADR
  0008) and register the same conceptual way — see `docs/EXTENDING.md`'s
  "Adding a new attack" section.
- `conflux-attacks` is intentionally excluded from `conflux-server`'s
  build — an attack implementation existing in the workspace must never
  become reachable from a production binary by accident.
