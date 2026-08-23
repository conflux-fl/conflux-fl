# 0001 — Two-axis configuration (topology × mode)

## Context
Conflux must support four deployment topologies (cross-silo, cross-device,
crowdsource, edge) that differ in network/domain shape, and two operating
postures (research, production) that differ in safety/reproducibility
requirements. Conflating these into one flat profile concept would force a
combinatorial explosion of profiles (e.g. `cross_silo_research`,
`cross_silo_production`, ...) every time either dimension grows.

## Decision
Split configuration into two disjoint axes that layer independently:

```
framework built-in fallback
    → topology profile   (cross_silo | cross_device | crowdsource | edge)
    → mode profile        (research | production)
    → explicit experiment-level override      [highest precedence]
```

Topology answers "what kind of participants and network?"; mode answers "am I
iterating on research, or running a live deployment?". Each axis owns a
disjoint set of parameters (see spec §9's reference table), so layering never
conflicts, and an explicit experiment-level override always wins regardless
of axis.

## Consequences
- Adding a new topology or mode never requires touching the other axis.
- Every config parameter must be classified as topology-owned, mode-owned, or
  neither (fixed/experiment-only) — see spec §9.
- Resolution order is fixed and must be explainable (see [[0007-explainable-config-resolution]]).
