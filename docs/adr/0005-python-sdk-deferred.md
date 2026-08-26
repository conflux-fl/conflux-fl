# 0005 — Python SDK and model distribution deferred

## Context
Two real product questions remain unresolved: how a model architecture is
introduced to a `ClientApp`, and how client code is distributed to
participants in a crowdsourced/edge deployment (pip package? container?
something the web application layer handles?). Neither is a framework-design
question in the sense the rest of the spec addresses — they depend on
product decisions not yet made.

## Decision
Explicitly defer both the Python `ClientApp` SDK design and the
model-distribution mechanism (spec §7, Open Item 3 in §11). In their place, a
**stub Python client** — fixed dummy weights, no PyTorch dependency — stands
in for end-to-end pipeline testing, permitted only in research mode via
`allow_stub_client` (see [[0001-two-axis-configuration]]).

## Consequences
- Phase 6 (spec §10) delivers the stub client and an end-to-end pipeline
  test in pull mode, not the real SDK.
- `allow_stub_client = false` in production means `conflux-server` refuses
  to start without a real `ClientApp` connection configured — no
  accidentally running a live deployment against the pipeline-testing stub.
- Real SDK and distribution design is a prerequisite for Phase 6 to move
  beyond the stub (tracked as Open Item 3, spec §11).

## Update (2026-08-23) — what would actually unblock this, framed as decisions rather than tasks

This ADR is still correctly deferred: nothing below is a decision made,
it's the decision tree that has to resolve before a Phase-numbered brief
could be written the way `phase-14-perclient-accounting.md` or
`phase-15-centered-clipping.md` could be (concrete deliverables against
an already-decided architecture). SDK design is different in kind from
those — it's blocked on **product** answers, not technical ones, so this
update only names the questions, per this ADR's own original framing.

Three separable questions, currently conflated under "the SDK":

1. **Model architecture handoff** — how does a `ClientApp` learn what
   model to train? Candidates: (a) the experiment config carries a
   Python import path (`model_module = "my_package.models:MnistCNN"`) —
   `ClientApp` imports it directly, zero new wire-format need, but
   requires the model code to already be installed in the client's
   Python environment; (b) the server ships a serialized model
   definition (e.g. TorchScript or ONNX) over the existing local gRPC
   `TaskResponse` — no separate install step, but `conflux-node`'s wire
   format would need a new field, and Conflux would be taking on
   responsibility for a portable serialized-model story it doesn't have
   today. (a) is far cheaper and consistent with ADR 0004's boundary
   (Rust stays opaque to model architecture) — this update's
   recommendation, not yet a decision.
2. **Client code distribution** — how does a participant get the
   `ClientApp` code at all, in `crowdsource`/`edge` topologies where
   participants aren't pre-provisioned machines? A `cross_silo` deployment
   can assume out-of-band installation (an institution's own ops team
   deploys it); `crowdsource` can't. This is the harder of the two
   questions and the one most clearly outside this codebase's boundary —
   candidates (a pip package a participant installs themselves, a
   container image, something the—not-yet-designed—web application layer
   pushes) are a product/deployment-model decision, not something this
   repo can resolve unilaterally.
3. **What the SDK actually wraps** — assuming (1) and (2) resolve, the
   `ClientApp` interface itself (today: fixed dummy weights, no PyTorch)
   needs a real shape: a `train(weights: list[float]) -> (delta: list[float],
   num_samples: int)`-style callback contract is the minimal version
   consistent with the current stub's behavior and the existing
   `TaskResponse`/`DeltaChunk` wire format (ADR 0004) — no proto change
   needed for this part specifically, unlike (1)'s option (b).

**Recommendation**: resolve (3) first — it's pure technical design,
answerable from this codebase alone, and unblocks writing a real
`ClientApp` base class usable in `cross_silo` deployments (where (1)/(2)
can stay "assume out-of-band installation" indefinitely, since that's
already how the existing E2E numpy/PyTorch examples work). Treat
(1)/(2) as `crowdsource`/`edge`-specific follow-on work, gated on product
scope this ADR still can't resolve — this keeps `cross_silo` from staying
blocked on decisions that are specific to the other two topologies.
