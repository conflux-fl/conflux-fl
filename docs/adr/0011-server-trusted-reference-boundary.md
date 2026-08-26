# 0011 — FLTrust/Zeno need server-side training: revisiting ADR 0004's boundary

**Status: proposed — pending project-owner review.** This is a scoping
ADR, not an implementation plan: it records the boundary question and a
recommendation, the way `docs/AGGREGATION_LANDSCAPE.md`'s "Update
(2026-08-23)" section already flagged was needed before FLTrust/Zeno
could move past being a name in a table.

## Context

FLTrust (Cao, Fang, Liu, Jia & Gong, 2021) and Zeno/Zeno++ (Xie, Koyejo
& Gupta, 2019/2020) are the only two aggregation methods in Conflux's
tracked landscape (`docs/AGGREGATION_LANDSCAPE.md` Category 3) whose
published algorithm requires the **server** to hold data and train (or
evaluate loss) on it: FLTrust trains its own reference update each round
on a small trusted root dataset; Zeno scores client updates by loss
improvement on a held-out server-side validation set. Both anchor their
robustness to an independently-computed signal rather than deriving it
from the client batch — architecturally the strongest fix pattern
`AGGREGATION_LANDSCAPE.md` identified for the Sybil/collusion problem
Category 2 methods share, and structurally immune to the batch-integrity
bug that document's Category 2 section documents.

But `docs/adr/0004-client-server-split-local-grpc.md` is explicit: Python
(PyTorch) stays entirely client-side; `conflux-server` is opaque to model
architecture by design, which is *why* the wire format is a flat
`f32[]` and why Step 2 (local training) is "the only FL step with zero
Rust-side algorithmic logic" (spec §8). FLTrust and Zeno both require the
server to run a forward/backward pass (FLTrust: a training step; Zeno: at
minimum a loss evaluation) — neither is possible under ADR 0004's
boundary as it stands today. This was first surfaced as a blocker in
`docs/phases/phase-13-reputation-reference-fix.md`'s "Revision history"
(its first draft assumed a trusted-reference reputation fix, then found
this exact conflict and rescoped around it), and flagged there as
needing "its own ADR revisiting ADR 0004" — this is that ADR.

## The actual tension, precisely

ADR 0004's boundary exists for a specific reason: keeping GPU/PyTorch
training entirely client-side is what lets `conflux-server` stay a thin,
topology-agnostic orchestrator that never needs to know what model
architecture it's coordinating. FLTrust/Zeno don't threaten that
boundary's *purpose* (the server still doesn't need to understand the
client's model architecture to hold a small trusted dataset and run
*some* forward pass against it) — but they do require the server process
to gain a real training/inference capability it has zero of today: no
PyTorch dependency, no GPU assumption, no model-loading code, nothing.
Adding that is not a small extension of `conflux-server`; it's a new
capability class the spec never scoped for the server side at all.

Three shapes this could take, none free:

1. **Server embeds a real training capability** (e.g. an `ort`/ONNX
   Runtime or `tch` (libtorch) dependency in `conflux-server` itself).
   Closest to the papers' literal design. Directly contradicts ADR
   0004's stated boundary — the server would no longer be opaque to
   model architecture, and would need GPU/runtime dependencies no other
   Conflux crate has. This is the option `phase-13`'s revision correctly
   refused to take on as a same-phase patch.
2. **A separate, optional sidecar process** (Rust or Python) that owns
   the trusted-dataset training/scoring, communicating with
   `conflux-server` over the same kind of local gRPC hop `conflux-node`
   already uses for its Python `ClientApp` (ADR 0004's own precedent —
   one schema, reused for a new hop). `conflux-server` stays opaque
   itself; a new, explicitly-optional component takes on the capability
   ADR 0004 keeps out of the server proper. Closest to preserving ADR
   0004's actual intent (the *server binary* stays training-free) while
   still making FLTrust/Zeno real.
3. **Don't build FLTrust/Zeno.** A legitimate option per this project's
   own faithful-catalog principle (`docs/adr/
   0008-cited-baseline-implementations.md`) — Conflux doesn't have to
   carry every published method, and these two are structurally the
   most expensive in the whole tracked landscape. Catalog completeness
   is a nice-to-have, not a requirement.

## Recommendation (not yet decided)

Option 2, if FLTrust/Zeno get prioritized at all: a
`conflux-trusted-reference` sidecar, gRPC-adjacent to `conflux-server`
the same way `conflux-node`'s local hop is adjacent to `conflux-server`'s
network hop, reusing `conflux-proto`'s existing flat-`f32[]` shape for
whatever it hands back (a reference update vector for FLTrust, a
per-client score vector for Zeno). This keeps `conflux-server` itself
unchanged — the sidecar is an optional process a deployer runs only if
they've configured `aggregator = "fltrust"` or `"zeno"`, mirroring how
`allow_stub_client` and `require_node_auth` are already optional,
mode-gated capabilities elsewhere in this codebase. `FLTrust`/`Zeno`
would each still be implemented as ordinary `Aggregator` family members
in `conflux-core` (per ADR 0002, faithfully matching their own papers'
definitions) — the sidecar only supplies the trusted-reference/score
signal each one's `aggregate()` call needs, the same way `conflux-net`
supplies network I/O to every aggregator today without either being
coupled to the other's internals.

This ADR does not decide whether to build FLTrust/Zeno at all (option 3
stays live), only what shape the work would take *if* prioritized. If
adopted, `crates/conflux-attacks`' own ADR 0010 precedent (a
dev-dependency-only crate, never a `conflux-server` dependency) is the
right model for keeping the sidecar equally optional and equally
uncoupled from the production binary's own dependency graph.

## Consequences (if this recommendation is later adopted)

- `conflux-server` gains zero new dependencies — the training/scoring
  capability lives entirely in a new, separate crate/process.
- FLTrust/Zeno become deployable only by operators who explicitly run
  the sidecar — consistent with this project's "opt-in, never a bolted-on
  universal default" principle (`docs/phases/
  phase-13-reputation-reference-fix.md`'s governing correction, and the
  standing [[feedback_faithful_catalog_not_defense_platform]] memory).
- A new local-gRPC hop pattern gets established (server ↔ sidecar), the
  third reuse of `conflux-proto`'s "one schema, multiple hops" design
  (ADR 0004) after server↔node and node↔`ClientApp`.
- Neither method's own `Aggregator` implementation is blocked on this
  ADR being finalized *first* — the trait-level work (matching each
  paper's own combine/scoring formula) can be drafted independently; only
  wiring up a *real* trusted dataset/training signal needs the sidecar
  question resolved.
