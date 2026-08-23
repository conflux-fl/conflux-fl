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
