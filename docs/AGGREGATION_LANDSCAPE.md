# Aggregation method landscape vs. the reputation-filter pipeline order

`docs/E2E_TESTING.md`'s "Real findings" section documents one concrete bug:
`conflux-reputation`'s pre-aggregation filter can be poisoned by a
round-1 outlier, starving whichever `robust` aggregator is configured of
the honest batch it needs. That was found against four shipped
aggregators (FedAvg, Krum, Trimmed Mean, Median). This doc generalizes
the question to the wider aggregation literature — roughly 18 real,
citable methods — because `docs/STATUS.md`'s "Next" section already lists
several future aggregators (Bulyan) and selectors, and the family pattern
(ADR 0002) is meant to make adding more of these cheap. Before extending
that pattern further, it's worth knowing *which* future methods this bug
class would silently defeat, and which need architecture the current
`averaging`/`robust` families don't have yet.

**Read this as prep for a phase brief, not a decision already made.**
Nothing here is implemented; it's the analysis that should precede
scoping the reputation fix and any new family members.

## The exact mechanism, precisely (for reference)

[`crates/conflux-server/src/round.rs:72`](../crates/conflux-server/src/round.rs)
computes `let reference = mean_vector(&decoded);` — the raw arithmetic
mean of the whole batch — and passes it into
[`conflux_reputation::filter_by_threshold`](../crates/conflux-reputation/src/lib.rs).
One useful detail this re-read surfaced: `ContributionScorer::score(&self,
update: &[f32], reference: &[f32])` already takes `reference` as an
argument the *caller* supplies — `CosineScorer` itself has no opinion on
how the reference is computed. **The bug is entirely in `round.rs`'s
one-line choice of reference, not in the trait shape.** Whatever fix gets
designed, it's a change to what gets passed at that call site, not a
`conflux-reputation` interface change — good news for how surgical the
eventual fix can be.

## Five categories, by how each method depends on batch integrity

### Category 1 — No inherent robustness; reputation was its *only* defense

| Method | Citation | Notes |
|---|---|---|
| **FedAvg** | McMahan et al., 2017 | Already shipped. Sample-count-weighted mean, no built-in Byzantine resistance at all. |

FedAvg was never claiming robustness itself — reputation filtering in
front of it was supposed to *be* the defense. The bug doesn't "defeat" a
robustness property FedAvg never had; it defeats the *only* protection an
undefended aggregator gets. Confirmed directly: undefended `fedavg`
collapsed to the same 0.3975 accuracy as `krum` under the same attack,
which is the expected (bad) outcome for this category, not a surprise.

### Category 2 — Batch-integrity-dependent robust aggregation (the exact bug already found — applies to ~9 of the 18)

These all share one assumption: the aggregator's *input batch* contains a
bounded fraction of Byzantine clients, and the aggregator's entire value
is in correctly separating honest from malicious **within that batch**.
If reputation has already discarded the honest updates before the
aggregator runs, every method in this category degrades to the same
failure mode — this is the generalization the earlier, narrower finding
was pointing at.

| Method | Citation | Family shape (ADR 0002) | Status |
|---|---|---|---|
| **Krum** | Blanchard, El Mhamdi, Guerraoui & Stainer, 2017 | `UpdateFilter` (selection-based) | Shipped |
| **Multi-Krum** | same | `UpdateFilter` | Shipped |
| **Trimmed Mean** | Yin, Chen, Ramchandran & Bartlett, 2018 | `CoordinateWiseRobustStatistic` | Shipped |
| **Median** | same | `CoordinateWiseRobustStatistic` | Shipped |
| **Bulyan** | El Mhamdi, Guerraoui & Rouault, 2018 | `FilteredAggregator<BulyanFilter, TrimmedMean>` — composes existing shapes, zero new plumbing (already noted in `docs/STATUS.md`) | Sketched, not built |
| **FABA** | Xia, Zhang, Yang, Shao & Yin, 2019 | `UpdateFilter` — iteratively drops the update farthest from the running mean, like a simpler Krum | Not built |
| **Divide-and-Conquer (DnC)** | Shejwalkar & Houmansadr, 2021 | `UpdateFilter`, but the filtering *logic* (random coordinate subsampling + top-singular-eigenvector outlier scoring) is spectral, not distance-based — the trait shape fits, the internals are a genuinely different algorithm family from Krum's pairwise distances | Not built |
| **Geometric Median / RFA** | Pillutla, Kakade & Harchaoui, 2019/2022 | **Doesn't cleanly fit either existing trait.** It's a *whole-vector* robust statistic (Weiszfeld's algorithm, jointly over all coordinates) — `CoordinateWiseRobustStatistic` is explicitly per-coordinate-independent, which loses the cross-coordinate structure geometric median preserves. Needs a third trait shape, e.g. `RobustVectorStatistic: fn combine(&self, updates: &[Vec<f32>]) -> Vec<f32>` | Not built — flagged as a real gap in the current two-trait design |
| **Centered Clipping (CClip)** | Karimireddy, He & Jaggi, 2021 | Doesn't fit either trait either — clips each update around a **server-held reference vector that persists and updates across rounds** (not recomputed fresh from each round's batch alone). Closer in shape to a stateful transform than either existing family member | **Shipped** (Phase 15) — `CenteredClippingAggregator` in `temporal.rs`, config name `centered_clipping`; used `temporal.rs`'s cross-round state pattern, no new trait needed |

All nine assume the batch handed to them is representative. Pre-filtering
that batch with a scorer whose own reference can be skewed by the same
attacker the aggregator is meant to catch doesn't just weaken these
methods — for the exact attack shape already reproduced (single
magnitude-dominant outlier, shared-checkpoint round), it can zero out
their effective input entirely, independent of which of the nine is
configured. That's the generalization worth internalizing: **this isn't
a Krum bug. It's a bug in what "robust aggregation" gets to see, and it
applies uniformly across this whole category.**

### Category 3 — Independent-trusted-reference defenses (structurally immune to this specific bug, and the strongest fix candidate)

| Method | Citation | Mechanism |
|---|---|---|
| **FLTrust** | Cao, Fang, Liu, Jia & Gong, 2021 | Server holds a small trusted root dataset, trains its *own* reference update each round, and scores/reweights clients by cosine similarity **and** magnitude alignment against that self-trained reference — not against anything derived from the client batch. |
| **Zeno / Zeno++** | Xie, Koyejo & Gupta, 2019 / 2020 | Server scores each update by whether it improves loss on a held-out server-side validation set, keeps the top-scoring subset. Also anchored to an independent, server-controlled signal. |

These two matter more to this doc's central question than any of the
nine above, because they show the *actual* fix pattern in the published
literature: don't derive the filtering reference from the batch you're
trying to filter. `CosineScorer`'s trait signature already accepts an
arbitrary `reference: &[f32]` — so **the architecturally strongest fix
isn't "use a robust statistic of the batch as the reference" (median
instead of mean), it's "use a reference the batch can't influence at
all,"** the way FLTrust does. A robust-statistic reference (Category 2's
own methods, ironically) is still *derived from the batch* and still
breaks once the Byzantine fraction crosses the ~50% breakdown point any
batch-only statistic has. A trusted-root-dataset reference doesn't have
that ceiling. This is worth weighing against the three candidate fixes
`docs/STATUS.md` already lists — it's a fourth, and arguably better, one:
**give `conflux-reputation` (or a new `robust` family member built the
same way) a small server-held trusted reference, shared infrastructure
both could use**, rather than reinventing "robust reference" twice.

### Category 4 — Needs new pipeline architecture, independent of the robustness question

These aren't about Byzantine defense at all, and the reputation bug
doesn't apply to them the same way — but they're common enough in the
literature that a "future aggregators" architecture pass should account
for them now rather than retrofitting later.

| Method | Citation | What it actually needs |
|---|---|---|
| **FedNova** ✅ *built 2026-09-01* | Wang, Liu, Liang, Joshi & Poor, 2020 | Per-client local-step-count-normalized weighting. ~~Fits `AveragingWeighting` cleanly (a new member, `StepNormalizedWeighting`)~~ — **this was wrong, and building it is what showed why.** Expanding the update and collecting terms gives `x·(1 − τ_eff·S) + τ_eff·Σ(p_k/τ_k)·x_k` with `S = Σ p_k/τ_k`; that is a weighted average of the clients' weights only if `τ_eff·S = 1`, and by Cauchy–Schwarz `(Σ p_k τ_k)(Σ p_k/τ_k) ≥ 1` with equality **iff every τ_k is equal** — precisely when FedNova degenerates to FedAvg. So the one regime where the `AveragingWeighting` formulation is correct is the regime where the method does nothing. FedNova needs `x_t`, and is therefore a **stateful** aggregator (ADR 0012's `Mutex` pattern), not a weighting. Shipped in `optimization.rs` as `FedNovaAggregator`. |
| **FedAdam / FedYogi / FedAdagrad ("FedOpt")** | Reddi et al., 2020 | A server-side adaptive optimizer applied to the aggregated delta as a pseudo-gradient, with first/second-moment state that **persists across rounds**. This isn't a new `AveragingWeighting`/`UpdateFilter`/`CoordinateWiseRobustStatistic` member — it's a *post-aggregation* stage that wraps around whatever base `Aggregator` is chosen (compatible with FedAvg or any `robust` member equally). The current `Aggregator` trait's `aggregate(&self, updates) -> Vec<f32>` is stateless and per-round; FedOpt needs an `Aggregator` (or a new wrapping stage) that owns mutable state across calls. |
| **SCAFFOLD** ✅ *built 2026-09-01* | Karimireddy, Kale, Mohri, Reddi, Stich & Suresh, 2020 | Needs each client to also send a **control-variate delta**, same dimensionality as the model delta, combined into the correction on both client and server. Took the `conflux-proto` extension rather than the packing convention. ADR 0012 supplied the *upward* half (`ClientDelta.control_variate`); building it found the downward half was missing entirely — a client that never learns `c` cannot compute `(c − c_i)`, so `TaskResponse.control_variate` and the `Aggregator::control_variate` hook were added. **SCAFFOLD is the only method in the catalog whose algorithm requires the server to send state down to clients.** |
| **Centered Clipping** | (Category 2 above) | Listed there for its robustness angle; also belongs here structurally — same persistent-server-state need as FedOpt. |

The pattern across this whole category: **the `Aggregator` trait as it
exists today (`fn aggregate(&self, updates: &[ClientDelta]) ->
Result<Vec<f32>, AggregatorError>`) assumes no state survives between
rounds and no wire-format fields beyond `num_samples`.** Three real,
popular methods (FedNova, FedOpt-family, SCAFFOLD) need one or both of
those assumptions relaxed. Worth deciding *now*, before more `averaging`
family members land, whether cross-round aggregator state becomes a
first-class capability (e.g. `&mut self` on `aggregate`, or a separate
`ServerOptimizer` stage the round pipeline calls after aggregation) —
retrofitting statefulness onto an established stateless trait later is
more disruptive than designing for it once.

### Category 5 — Needs zero server/framework change (reassurance, not a gap)

| Method | Citation | Why it's already supported |
|---|---|---|
| **FedProx** ✅ *built 2026-09-01* | Li, Sahu, Zaheer, Sanjabi, Talwalkar & Smith, 2018/2020 | The proximal term (`μ/2‖w − w_global‖²`) is added to the **client's local loss function** during local training — a pure `ClientApp`-side change. The server never sees anything different: still one flat `f32` delta per round, same wire format (ADR 0004). Now implemented where it belongs: `train_steps(..., mu=)` in the MNIST harness, exposed as `--mu`. Measured to cut drift from `w_t` by 62% at `μ = 1.0`. There is deliberately **no** `aggregator = "fedprox"` — its server half *is* FedAvg — but naming it now returns a dedicated error saying exactly that, rather than "unknown aggregator". |

Good to have at least one entry confirming the family-pattern extension
work isn't needed for everything — some real, popular methods are
already fully expressible without touching `conflux-core` at all.

### Also worth flagging: methods that stress other design assumptions

Two more, briefly, because they're popular enough to come up eventually
and don't fit neatly into the above five:

- **SignSGD with majority vote** (Bernstein, Wang, Azizzadenesheli &
  Anandkumar, 2018; Byzantine analysis in Bernstein et al., 2019) —
  aggregates the *sign* of each client's gradient via majority vote,
  discarding magnitude entirely. This challenges the wire format's
  implicit assumption (a flat `f32[]` where magnitude carries
  information) more than it challenges the reputation/aggregation
  ordering — a genuinely different case from everything above, and a
  good stress test for whether `conflux-proto`'s "one schema for
  everything" (ADR 0004) holds up to a fundamentally different update
  representation.
- **Personalization methods** (FedPer — Arivazhagan et al., 2019; Ditto —
  Li, Hu, Beirami & Smith, 2021; pFedMe — T Dinh, Tran & Nguyen, 2020) —
  produce *per-client* models rather than one global broadcast model;
  some parameters stay local and never aggregate at all. Orthogonal to
  the robustness discussion, but relevant to "many more aggregators
  later": these need the pipeline to know that not every dimension of
  the weight vector is meant to be combined the same way, which today's
  "one flat vector, fully aggregated, fully broadcast" model doesn't
  represent.

## What this means architecturally, concretely

1. **The reputation fix should target the reference computation, not any
   individual aggregator.** Category 2 shows the blast radius is the
   entire `robust` family, present and future — fixing `round.rs`'s
   `mean_vector` call site (or replacing what it feeds
   `filter_by_threshold`) once is strictly better than any per-aggregator
   workaround.
2. **FLTrust/Zeno (Category 3) suggest a stronger fix than "robust
   statistic of the batch."** A trusted server-held reference has no
   Byzantine-fraction breakdown point; a batch-derived robust statistic
   (even median) still does. Worth designing the reputation fix around a
   shared "trusted reference" primitive rather than just swapping mean
   for median — and worth noting that primitive could *also* become a new
   `robust` family member (FLTrust itself), reusing the same
   infrastructure instead of building it twice.
3. **The current two `robust`-family traits (`UpdateFilter`,
   `CoordinateWiseRobustStatistic`) don't cover everything already in the
   literature.** Geometric Median needs a whole-vector statistic shape;
   Centered Clipping needs persistent cross-round state. Both are real,
   cited, popular methods — worth deciding the trait taxonomy's next
   shape before, not after, someone needs to bolt one on urgently.
4. **`AveragingWeighting`/`Aggregator` as currently defined can't express
   FedNova, FedOpt, or SCAFFOLD without either a proto change (FedNova,
   SCAFFOLD) or relaxing the stateless-per-round assumption (FedOpt,
   Centered Clipping).** None of this needs to happen now, but the
   `Aggregator` trait's signature is exactly the kind of thing that's
   cheap to future-proof today (e.g. deciding whether cross-round state
   is ever allowed) and expensive to retrofit after several more
   stateless-only family members exist.
5. **Not everything needs core-framework work.** FedProx is a reminder
   that the client-side/server-side split (ADR 0004) already buys a lot
   of algorithm flexibility for free — worth keeping in mind so the
   family-pattern's scope doesn't grow to cover things that were never
   the server's problem.

## Summary table (all 18)

| # | Method | Category | Needs |
|---|---|---|---|
| 1 | FedAvg | 1 — undefended | Shipped |
| 2 | Krum | 2 — batch-dependent robust | Shipped |
| 3 | Multi-Krum | 2 | Shipped |
| 4 | Trimmed Mean | 2 | Shipped |
| 5 | Median | 2 | Shipped |
| 6 | Bulyan | 2 | **Shipped** (2026-08-23) — composed existing shapes exactly as predicted |
| 7 | FABA | 2 | **Shipped** (2026-08-23) |
| 8 | Divide-and-Conquer | 2 | **Shipped** (2026-08-23) — `UpdateFilter`, new `top_singular_vector` power-iteration helper |
| 9 | Geometric Median / RFA | 2 | **Shipped** (2026-08-23) — new `RobustVectorStatistic` trait + `VectorRobustAggregator<S>` |
| 10 | Centered Clipping | 2 + 4 | New trait shape + cross-round state — `temporal.rs`'s `Mutex`-based state (built for FoolsGold, below) is now a concrete precedent for the cross-round part |
| 11 | FLTrust | 3 — trusted-reference | New trait shape + a trusted-reference primitive — still needs its own ADR-0004-revisiting scoping (Phase 13's "Revision history") |
| 12 | Zeno / Zeno++ | 3 | Same trusted-reference primitive as above |
| 13 | FedNova | 4 — needs new plumbing | `conflux-proto` field (local step count) |
| 14 | FedAdam/Yogi/Adagrad | 4 | Cross-round server optimizer state |
| 15 | SCAFFOLD | 4 | `conflux-proto` payload extension |
| 16 | FedProx | 5 — already supported | Nothing (client-side only) |
| 17 | SignSGD majority vote | wire-format stress test | Different update representation entirely |
| 18 | FedPer / Ditto / pFedMe | personalization, orthogonal | Partial-aggregation model the pipeline doesn't have yet |
| 19 | Median-of-Means | 2 (not in the original 18 — added during implementation) | **Shipped** (2026-08-23) — `CoordinateWiseRobustStatistic`, groups by array position |
| 20 | FoolsGold | **new Category 6 — cross-round/temporal** (not in the original taxonomy; see `docs/research/temporal-consistency-aggregation.md`) | **Shipped** (2026-08-23) — first member of a new `temporal.rs` module, `Mutex`-based per-client history, the first Conflux aggregator with real cross-round state |

## Where this leaves the reputation fix

Nothing above changes the recommendation from the earlier finding — fix
`round.rs`'s reference computation before adding more `robust` family
members, since every one of them inherits the current vulnerability
automatically. What this analysis adds: the fix is worth designing around
a reusable trusted-reference concept (Category 3's lesson) rather than
just a more robust batch statistic, since that same primitive pays for
itself again the moment FLTrust or Zeno gets built. Still real design
work deserving its own phase brief, not a same-session patch — see
`docs/STATUS.md`'s "Next" section.

## Update (2026-08-23) — the trusted-reference recommendation above is corrected in the phase brief

`docs/phases/phase-13-reputation-reference-fix.md` (the scoping brief
this section called for) found a real problem with the trusted-reference
recommendation two paragraphs up: **FLTrust and Zeno both require the
server to train its own reference update on real data.** Conflux's server
never trains anything — that's not an incidental gap, it's ADR 0004's
central boundary (`conflux-server` is opaque to model architecture by
design, which is *why* the wire format is a flat `f32[]`). Adopting a
trusted-reference design isn't a reputation-module change under that
boundary; it's new server-side ML capability that would need its own ADR
revisiting 0004 first. The phase brief scopes Phase 13 to what's
achievable without that: reject non-finite submissions outright (a
distinct bug this doc's Category-2 analysis didn't anticipate — see the
brief and `docs/E2E_TESTING.md`'s finding 3, found via this session's
Dirichlet non-IID testing, after this doc was written), and replace the
raw-mean reference with a coordinate-wise median reusing
`conflux-core::MedianStatistic` — real, in-scope improvements, explicitly
not claimed to close the general Byzantine-fraction ceiling Category 3
describes. That larger question stays open, correctly identified above,
just not being solved by Phase 13.

## Update (2026-08-23, second) — this whole doc's framing was too defense-oriented

Project-owner guidance corrected the premise this document was written
under: Conflux's purpose is a **faithful, extensible catalog of every
published aggregation method**, not a maximally-defended system. Each
method should behave exactly as its cited paper defines it — Krum should
be literal Krum — with the framework never modifying a method's own
behavior, and reputation/trust mechanisms treated as a property of
whichever specific method defines one (FLTrust, Zeno), not something
imposed generically in front of every aggregator. Priority is keeping
the family pattern (ADR 0002) simple so more methods keep being cheap to
add, not minimizing attack surface.

Under that lens, `docs/phases/phase-13-reputation-reference-fix.md` was
revised a second time: **the fix is not "make `conflux-reputation`'s
filter more robust" (the update immediately above), it's "stop making
that filter mandatory."** `CosineScorer` applied unconditionally in
front of every aggregator was itself the mistake — no cited paper in
this document's own tables asks for an extra uncited filter ahead of
Krum, Trimmed Mean, or Median. The revised Phase 13 makes reputation
filtering opt-in (off by default), so every method's default behavior
matches its paper with zero interference, and drops the coordinate-wise
median plan entirely (solving robustness for a mandatory gate that no
longer exists). The non-finite (`NaN`/`Inf`) rejection fix from the
first update still stands — that's a plain correctness bug independent
of whether reputation filtering is on by default.

This document's Category 2/3 taxonomy (which future methods this bug
class would affect, which are structurally immune) remains accurate and
useful groundwork for *if and when* FLTrust/Zeno get prioritized — each
would be built as its own self-contained aggregator implementing its
own cited trust mechanism faithfully, not as a `conflux-reputation`
extension. What no longer holds is this document's implicit framing that
Conflux's job is to defend every configured aggregator against every
listed attack by default — it isn't. See `docs/phases/
phase-13-reputation-reference-fix.md`'s "Revision history" for the full
account.

## Update (2026-08-23, third) — a new Category 6, and six methods shipped

Six of this table's remaining entries (Bulyan, FABA, Divide-and-Conquer,
Geometric Median, plus Median-of-Means and FoolsGold, not in the
original 18) are now built — see the summary table above and
`docs/STATUS.md`'s "Done" entry for that date. FoolsGold surfaces a gap
this document's original taxonomy didn't have a category for: **every
one of the ten pre-existing methods, across Categories 1–5, judges a
round's batch in isolation** — none have memory of prior rounds. That's
a real, distinct axis from "which geometric signal does the filtering,"
worth calling **Category 6 — cross-round/temporal**. FoolsGold is its
first member; `docs/research/temporal-consistency-aggregation.md` is a
full research proposal exploring whether that axis, generalized, closes
this document's Category 1/2 attack-adaptivity gap and the non-IID
fairness problem simultaneously — read that document for the deeper
analysis; it supersedes this doc as the place gap-analysis work
continues from here. A second Category-6 member, Deviation Stability
Scoring (DSS), is now also built and validated — see that document's
§5.5/§6.4 for its own real, measured tradeoffs.

## Update (2026-08-23, fourth) — this table's remaining gaps are now scoped, not just identified

Every remaining "Not built" row in the summary table above now has a
concrete planning document, closing the loop this doc opened:

- **Centered Clipping** (row 10) — `docs/phases/
  phase-15-centered-clipping.md`. Buildable now, independent of the
  proto-extension ADR below — needs cross-round state only,
  `temporal.rs`'s `Mutex`-based pattern (proven twice over, by FoolsGold
  and DSS) is the direct precedent.
- **FLTrust / Zeno** (rows 11–12) — the "still needs its own
  ADR-0004-revisiting scoping" this table already flagged is now written:
  `docs/adr/0011-server-trusted-reference-boundary.md`. Recommends an
  optional sidecar process (keeping `conflux-server` itself training-free,
  preserving ADR 0004's actual boundary) over embedding a training
  capability in the server binary directly.
- **FedNova / FedOpt-family / SCAFFOLD** (rows 13–15) — the shared
  plumbing question ("what this means architecturally" point 4, above)
  is now resolved as a proposed decision:
  `docs/adr/0012-stateful-aggregator-and-proto-extension.md`. Keeps
  `Aggregator::aggregate`'s `&self` signature (the `temporal.rs` pattern
  generalizes rather than requiring `&mut self`), adds two `optional`
  `ClientDelta` fields (`local_steps`, `control_variate`) additively —
  every existing producer of `ClientDelta` is unaffected.

None of these four are implemented yet — each remains a proposal
awaiting project-owner review, per those documents' own "Status" lines —
but the analysis-to-scoping gap this table's rows described is now
closed for every entry that had one.

## Update (2026-08-30, fifth) — Centered Clipping shipped

Row 10 is no longer a gap. `CenteredClippingAggregator`
(`crates/conflux-core/src/temporal.rs`, Phase 15) is registered as
`centered_clipping` and selectable from config like every other shipped
method.

Two things this row's original analysis got right, confirmed by
building it:

- **It needed no new trait.** The row predicted CClip "doesn't fit
  either trait" and would need a shape of its own. In the event, it
  needed *no* family trait at all — it implements `Aggregator` directly,
  the way `FoolsGoldAggregator` and `DssAggregator` already do. The
  `temporal` family is not a trait, it is a place where methods that
  own cross-round state live; that was the right precedent to point at.
- **It needed no `conflux-proto` change**, as the phase brief predicted,
  so it landed independently of ADR 0012.

One thing worth adding to the row's characterization, learned from
measuring it (`docs/research/temporal-consistency-aggregation.md`
§5.10): CClip is the only method in this table whose single tunable
bounds the **attack** and the **convergence rate** with the same
number. Every selection-based method's parameter (`byzantine_fraction`)
trades false positives against false negatives; `τ` instead trades
safety against speed, which is a different kind of knob and behaves
differently under mis-tuning — mis-tuned low it does not become
*wrong*, it becomes *slow*. That is a genuine robustness property (its
worst case is bounded by construction rather than by an assumed
attacker count) and a genuine operational cost, and it is not visible
from the taxonomy alone.

Remaining "Not built" rows are unchanged: FLTrust/Zeno (ADR 0011) and
FedNova/FedOpt/SCAFFOLD (ADR 0012).

## Update (2026-08-31, sixth) — FLTrust shipped; Category 3 is open

Rows 11–12 ("still needs its own ADR-0004-revisiting scoping") no longer
describe FLTrust. ADR 0011 was accepted as **option 2** — a separate,
optional `conflux-trusted-reference` sidecar — and FLTrust now ships as
an ordinary `Aggregator` family member in a new `trusted` family.

What this changes about the analysis above, in one line: **the ceiling
Category 2 was written to describe has a way over it now.** Every method
in Categories 1 and 2 derives "normal" from the batch, so a colluding
majority is normal by construction — which this document identified, and
which `docs/research/` §5.1 then measured. FLTrust never asks the batch:
its reference comes from data no client contributed to, so a unanimous
batch of attackers is scored against the same reference an honest one
would be. `conflux-core`'s own test suite asserts exactly that case
(`a_colluding_majority_does_not_win`).

The cost is an assumption swapped, not removed. FLTrust's guarantee is
exactly as good as its root dataset: trained on unrepresentative data,
the reference points somewhere honest clients do not, and `ReLU` zeroes
*them*. That is a different failure mode from the ones Category 2 has,
not a strictly smaller one — worth saying plainly, because
"structurally immune to the Sybil problem" is easy to read as "better".

**Zeno remains unbuilt.** The sidecar implements and serves its scoring
RPC, and a test exercises it over the real hop, but no `Aggregator`
consumes it yet — its combine (score, then keep a top-scoring subset) is
its own phase brief. Option 3 ("don't build it") stays live for Zeno.

## Update (2026-09-01, seventh) — the optimization family, and FLANDERS

Two additions that change what this document's own gap analysis says.

### Category 4 is no longer empty: FedAdagrad / FedAdam / FedYogi

Category 4 (cross-round state and per-client extra fields) listed
FedNova, SCAFFOLD and FedOpt as blocked on plumbing. ADR 0012 landed
that plumbing, and **FedOpt is now built** — all three variants of Reddi
et al. (2021) Algorithm 2, as a new `optimization` family.

This closes what was, measured against every comparable framework, the
largest hole in Conflux's catalog. The robust families ship ten methods
where Flower's built-in strategies ship five; the optimization family
shipped **zero** where Flower ships six. Adaptive server optimization is
the axis that makes federated training converge on non-IID data, and it
was entirely absent.

FedNova and SCAFFOLD remain unbuilt, but their blocker is gone —
`local_steps` and `control_variate` are both on the wire (ADR 0012).
What FedNova now needs is a client that populates its field, which is
the ADR 0005 SDK question, not an aggregation question.

### Category 2's ceiling has a second answer, and it is not a strict improvement

Category 3 (trusted-reference) was identified here as the structural
answer to the Sybil/collusion ceiling, and FLTrust shipped under ADR
0011. **FLANDERS** (Gabrielli, Belli, Matrullo, Miori & Tolomei, 2024,
arXiv 2303.16668) is a second answer of a different kind: it keeps the
cross-round history of each client and scores against a matrix
autoregressive forecast, so like the Category 3 methods it does not ask
the batch — but unlike them it needs no trusted data.

It now ships (`conflux-core/src/flanders.rs`), paired with Krum per its
own `ϕ`. Two things this document should record, both measured in
`docs/research/` §5.14 rather than argued:

- **It is not a strict improvement over the Category 1/2 methods.**
  Paired with FedAvg it scores *worse than undefended FedAvg* against
  every Sybil attack tested (24.2 vs 17.0 at 20% malicious). The reason
  is structural and now pinned as a unit test: a colluder that repeats
  itself is the most forecastable client in the batch, so a
  forecast-consistency filter keeps it and drops the noisier honest
  majority. Its own paper's evaluation uses attacks that perturb or
  optimize, where that failure mode cannot arise.
- **Its headline regime did not reproduce.** Against an adaptive
  attacker at 60% malicious it scored 1901.7 where undefended FedAvg
  scored 1659.1. Reported as what the numbers show, at `dim = 3`, on one
  attack family — not as a refutation of the paper's own results.

The useful generalization for this landscape: **a cross-round defense
must choose what "normal" means over time**, and the two available
choices fail on opposite attacks. Forecast-consistency (FLANDERS)
rewards clients that repeat themselves, so stable collusion defeats it.
Deviation-*instability* (DSS) requires clients to vary, so stable
collusion defeats it too — for the opposite reason. Neither is a
general answer, and the trusted-reference family (Category 3) remains
the only one that sidesteps the question entirely, at the cost of
needing trusted data.
