# Conflux — Multi-Session Development Plan

**Purpose:** This document exists because the spec (`conflux-spec-v1.md`) is too large for any single chat window to hold alongside real implementation work. It defines how to split development across independent sessions — separate chat windows, Claude Code sessions, or both — without losing decisions made in earlier sessions.

---

## 1. The Core Problem, Stated Plainly

Nothing carries between two separate chat windows automatically — not even inside the same Claude Project, conversation history does not transfer between chats; only the **knowledge base** and **instructions** you've explicitly saved do. So the plan below has one governing rule:

> **Every decision that matters must live in a file, not in a chat transcript.** If it's not written down somewhere the next session will read, it doesn't exist for that session.

This is what the ADR log (§3) and phase briefs (§4) below are for.

---

## 2. Recommended Tooling Split

| Activity | Tool | Why |
|---|---|---|
| Spec discussion, architecture decisions, naming, planning (what we've been doing) | **Claude Project** (claude.ai) with the spec + ADRs uploaded as knowledge | Good for reasoning/discussion; knowledge base persists across chats in the project without re-upload |
| Actual crate implementation, writing Rust, running `cargo test` | **Claude Code**, pointed at the repo | Reads `CLAUDE.md` and the repo itself at the start of every session — no manual context-pasting needed; git commits become the durable record of what's done |

Concretely: keep a Claude Project called something like "Conflux — Architecture" for spec evolution (upload `conflux-spec-v1.md` and the ADR files below to its knowledge base now). Do implementation work in Claude Code against the actual `conflux/` git repository, with the phase briefs and `CLAUDE.md` living in that repo.

---

## 3. Architecture Decision Records (ADR Log)

Extract the decisions made across this whole spec process into individual files under `docs/adr/`. Each is short — one page — capturing *what* was decided and *why*, so a future session (or a new contributor) never needs to re-read the full conversation history to understand a constraint. Initial index, drawn directly from this spec's history:

| # | Title | One-line summary |
|---|---|---|
| 0001 | Two-axis configuration (topology × mode) | Domain shape and safety posture are orthogonal; layered precedence resolves both against explicit overrides |
| 0002 | Family pattern for aggregation and privacy | New published methods extend a shared base (`AveragingWeighting`, `RobustSelection`) instead of reimplementing `Aggregator`/`PrivacyEngine` |
| 0003 | No multi-tenancy | One server process = one experiment; multi-experiment orchestration is an application-layer concern |
| 0004 | Client/server split via local gRPC | `conflux-node` (Rust) hands training to a Python `ClientApp` over loopback gRPC, same schema as the network hop |
| 0005 | Python SDK and model distribution deferred | Explicitly out of scope until a product decision is made; stub client stands in for pipeline testing |
| 0006 | `Global` epsilon accounting for v1 | Chosen over `PerClient` for a faster first working prototype; `PerClient` deferred to Phase 8 |
| 0007 | Explainable config resolution is mandatory | Every resolved parameter logs its source (CLI/env/file/profile/fallback); format configurable, JSON default in production |
| 0008 | Cited baseline implementations | `UniformRandomSelector` (McMahan et al., 2017) and `GaussianClippingPrivacy` (Abadi et al., 2016; Geyer et al., 2017) are the one shipped member of each family, docstring-cited |
| 0009 | Project name: Conflux | Chosen over Alloy (collides with `alloy-rs`, the dominant Rust Ethereum library) and Crucible (name taken on crates.io) |

Write each as `docs/adr/000N-title.md` with just: **Context** (what problem existed), **Decision** (what was chosen), **Consequences** (what it rules out or commits you to). Keep each under a page — the point is fast orientation, not a full record of the discussion that led there.

---

## 4. Phase-to-Session Mapping

Using the phased plan from the spec (§10), here's how to split it across sessions, with dependency and parallelizability noted:

| Session | Phase(s) | Depends on | Can run in parallel with |
|---|---|---|---|
| S0 | Phase 0 — workspace scaffold, `conflux-proto` | Nothing | — (do first, blocks everything) |
| S1 | Phase 1 — `conflux-config`, `conflux-registry` | S0 | — |
| S2a | `conflux-store` | S1 | S2b, S2c, S2d |
| S2b | `conflux-selector` | S1 | S2a, S2c, S2d |
| S2c | `conflux-privacy` (incl. `RdpAccountant`) | S1 | S2a, S2b, S2d |
| S2d | `conflux-reputation` | S1 | S2a, S2b, S2c |
| S3 | Phase 3 — `conflux-net` | S0, S1 | Can overlap with S2a–d (different crates) |
| S4 | Phase 4 — `conflux-buffer`, `conflux-core` | S3 (buffer needs net's types); core only needs S0 | — |
| S5 | Phase 5 — `conflux-server` integration | S2a–d, S3, S4 all complete | — (integration point, needs everything) |
| S6 | Phase 6 — `conflux-node` + stub client, e2e test | S5 | — |
| S7 | Phase 7 — production hardening | S6 | Individual hardening items (Redis, Postgres, mTLS) can split further and parallelize |
| S8 | Phase 8 — research expansion | S7 | Each new family member (Krum, Trimmed Mean, etc.) is its own small session |

The four Phase 2 leaf crates (S2a–d) are the best candidates for genuinely independent, parallel sessions — they don't depend on each other, only on Phase 1's trait definitions being settled. That's a natural place to start splitting across chat windows once S0/S1 land.

---

## 5. Phase Brief Template (what each session needs, and nothing more)

Store one of these per phase at `docs/phases/phase-N-<name>.md`. This is the file a new chat window or Claude Code session should be pointed at — it's deliberately narrow, not the whole spec:

```markdown
# Phase N — <crate/component name>

## Scope
One paragraph: what this session builds, and explicitly what it does NOT build
(so scope doesn't creep into a neighboring phase's territory).

## Inputs (what must already exist)
- Trait signatures this phase depends on (paste the exact `pub trait ...` block
  from the upstream crate, not a description of it)
- Relevant ADR numbers (link, don't re-explain)

## Deliverables
- File/module list
- Public API this phase must expose for downstream phases

## Test plan
(Pull directly from spec §5's per-crate test table, or the phase's own section)

## Definition of done
Checklist, copied/adapted from the spec's relevant "Definition of Done" section.
```

This keeps each session's context small and targeted — a session building `conflux-reputation` never needs to see `conflux-net`'s gRPC details, just the `ClientDelta` type it consumes and the `ContributionScorer` trait it implements.

---

## 6. `CLAUDE.md` (repo root, for Claude Code sessions)

```markdown
# Conflux

Rust federated learning framework. See docs/spec/conflux-spec-v1.md for full
architecture; docs/adr/ for why key decisions were made; docs/phases/ for the
current phase's scoped brief.

## Conventions
- Every fallible public fn returns a `thiserror`-derived enum, never `String`.
- New algorithm implementations register via `inventory::submit!` — see ADR 0002.
- Every resolved config parameter must log its source — see ADR 0007.

## Before starting work
1. Read docs/STATUS.md for what's done and what this session should tackle.
2. Read the relevant docs/phases/phase-N-*.md brief.
3. Check docs/adr/ for any ADR referenced in that brief.

## After finishing work
Update docs/STATUS.md with what shipped and what's next.
```

## 7. `STATUS.md` — the single source of truth across sessions

```markdown
# Conflux — Status

Last updated: <date>, Phase <N>

## Done
- [x] Phase 0 — workspace scaffold, conflux-proto

## In progress
- [ ] Phase 1 — conflux-config (started, provenance logging not yet implemented)

## Next
- Phase 1 completion, then split Phase 2 into 4 parallel sessions (S2a-d)

## Known deviations from spec
(anything a session had to diverge on, and why — keeps the spec and reality
from silently drifting apart)
```

This one file answers "where are we?" without anyone needing to reconstruct it from git log or chat history — update it at the end of every session, without exception.

---

## 8. Practical Next Step

1. Create the Claude Project, upload `conflux-spec-v1.md` and this development plan to its knowledge base.
2. Write the 9 ADRs from §3 as actual files.
3. Initialize the git repo, add `CLAUDE.md` and `docs/STATUS.md`.
4. Write the Phase 0 brief (`docs/phases/phase-0-workspace-proto.md`) and start a Claude Code session against it — that's the first piece of actual implementation, and everything else in this plan exists to make Phase 1 onward not require re-explaining Phase 0's decisions.
