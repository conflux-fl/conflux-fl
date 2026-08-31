# Phase 22 — the `optimization` family

**Status: partially shipped (2026-09-01).** FedAdagrad, FedAdam and
FedYogi are built and in the catalog. This brief covers what they are,
and scopes the four methods that remain.

## Why this family exists

Every other family in `conflux-core` answers *which clients should
count, and how much*. This one answers a different question: **given
whatever the batch aggregated to, how far should the server actually
move?**

FedAvg applies the aggregated update to the global model directly — a
server-side SGD step with learning rate 1. That is a choice, not a
necessity, and it is a poor one under non-IID data, where different
clients push different coordinates hard and a uniform step under- or
over-shoots most of them.

Measured against every comparable framework, this was Conflux's largest
catalog gap: ten robust methods against Flower's five built-in, and
**zero** optimization methods against its six.

---

## Shipped: FedAdagrad, FedAdam, FedYogi

Reddi, Charles, Zaheer, Garrett, Rush, Konečný, Kumar & McMahan (2021),
*Adaptive Federated Optimization* (ICLR), Algorithm 2. One
implementation, three variants differing in exactly one line:

```text
Δ_t = (1/|S|) Σ_i (x_i − x_t)                            pseudo-gradient
m_t = β1 m_{t-1} + (1 − β1) Δ_t                          first moment
v_t = v_{t-1} + Δ_t²                                     FedAdagrad
v_t = v_{t-1} − (1 − β2) Δ_t² sign(v_{t-1} − Δ_t²)       FedYogi
v_t = β2 v_{t-1} + (1 − β2) Δ_t²                         FedAdam
x_{t+1} = x_t + η · m_t / (√v_t + τ)
```

The per-coordinate division is the whole idea: a parameter receiving
consistently large updates gets a *smaller* effective step, one that has
barely moved gets a larger one.

**What differs between them, and why it matters:**

| | second moment | behavior |
|---|---|---|
| **FedAdagrad** | `v += Δ²`, no decay | The effective step only ever shrinks. Safe, and eventually too slow. Paper sets `β1 = β2 = 0` — no momentum, as classical Adagrad has none. |
| **FedAdam** | `v ← β2 v + (1−β2) Δ²` | Exponential decay. Recovers quickly after a large round, which is also its weakness: `v` can collapse toward a new small-gradient regime and produce a suddenly-huge step. |
| **FedYogi** | `v ← v ∓ (1−β2) Δ²` by sign | `v` moves *additively*, so it cannot collapse. More conservative than Adam after a shock — verified as a test, `yogis_second_moment_decays_more_slowly_than_adams`. |

Config: `CONFLUX_SERVER_LEARNING_RATE` (`η`) and `CONFLUX_SERVER_TAU`
(`τ`). `τ = 1e-3` is the paper's own value. **`η` has no honest default**
— it is the parameter the paper's entire experimental section sweeps per
task, so the builtin `1.0` is a placeholder in the same sense
`clip_radius = 1.0` is, and is documented as one.

---

## Not shipped, in the order they make sense

### 1. FedAvgM — server momentum. *Smallest remaining piece.*

**What it is.** FedAvg plus a momentum buffer on the server:
`v_t = β v_{t-1} + Δ_t`, then `x_{t+1} = x_t + η v_t`. Hsu, Qi & Brown
(2019), *Measuring the Effects of Non-Identical Data Distribution for
Federated Visual Classification*.

**Why it is nearly free here.** It is Algorithm 2 with the adaptive
denominator removed — same pseudo-gradient, same state pattern, same
`x_t` tracking. It is arguably a fourth `FedOptVariant` rather than a
new type, except that its update rule has no `v` at all, so it needs a
small branch rather than a new second-moment arm.

**Scope:** one variant, ~50 lines plus tests. **No proto change, no
config beyond reusing `server_learning_rate` and a new `β`.**

**Worth doing because** it is the standard baseline every FedOpt paper
compares against — including Reddi et al., whose own results table has a
FedAvgM column. Shipping FedOpt without it means Conflux cannot
reproduce that table.

### 2. FedProx — *a client-side method, and that is the whole story.*

**What it is.** Li, Sahu, Zaheer, Sanjabi, Talwalkar & Smith (2020),
*Federated Optimization in Heterogeneous Networks*. Adds a proximal term
`(μ/2)‖w − w_t‖²` to **the client's local loss function**, penalizing
drift from the global model during local training.

**The important part: its server-side aggregation is plain FedAvg.**
There is nothing to implement in `conflux-core`. Conflux already
"supports" FedProx in the only sense that matters — a client that adds
the proximal term to its own objective and submits the result gets
FedProx, today, with `aggregator = "fedavg"`.

**What it actually needs** is a way for the server to *tell* clients to
use `μ`, and a client that honors it. That is the ADR 0005 Python SDK
question, not an aggregation question. Listing FedProx as a missing
aggregator would be a category error, and `AGGREGATION_LANDSCAPE.md`
already files it under Category 5 for exactly this reason.

**Scope:** a config key (`proximal_mu`) plumbed to clients, plus SDK
work. **Blocked on ADR 0005.**

### 3. QFedAvg — fairness-weighted averaging

**What it is.** Li, Sanjabi, Beirami & Smith (2020), *Fair Resource
Allocation in Federated Learning*. Re-weights client contributions by
their **loss** raised to a power `q`, so clients the model serves badly
pull harder. `q = 0` recovers FedAvg; larger `q` trades mean accuracy
for a more uniform accuracy *distribution*.

**What it needs that does not exist.** The client's local loss. That is
not on the wire: `ClientDelta` carries `weights`, `num_samples`, and —
since ADR 0012 — `local_steps` and `control_variate`. QFedAvg needs a
fifth field, `optional float local_loss`.

**The good news** is that ADR 0012 established exactly how to add one,
and the three-edit recipe is written down in `EXTENDING.md`: the
`.proto` message pair, the reassembly in `submit_delta`, and the byte
count in `conflux-net`. `local_loss` is a scalar, so it follows
`local_steps`'s convention exactly — repeated per chunk, read from the
first to arrive.

**Scope:** one proto field + one aggregator. **Unblocked, and a good
first exercise of ADR 0012's recipe by someone who did not write it.**

**Worth doing because** it is the only fairness-oriented method in the
tracked landscape, and this project has already measured a
robustness–fairness tension it has no method to address (§5.3, §5.9).

### 4. FedNova and SCAFFOLD — plumbing done, clients missing

Both were unblocked by ADR 0012 and neither is built.

**FedNova** (Wang, Liu, Liang, Joshi & Poor, 2020) normalizes each
client's update by its local step count, so a client that ran 50 local
steps does not out-vote one that ran 5. `local_steps` is on the wire and
reassembles. The aggregator itself is a short `AveragingWeighting` impl
— genuinely small.

**SCAFFOLD** (Karimireddy, Kale, Mohri, Reddi, Stich & Suresh, 2020)
maintains control variates on both sides to correct client drift.
`control_variate` is on the wire and reassembles in `chunk_index` order.
The server side is tractable; **the client side is real work** and needs
a client that maintains its own variate across rounds.

**Both are blocked on the same thing FedProx is**: a client that
populates the field. The server-side halves are ready.

---

## Suggested order

1. **FedAvgM** — smallest, and needed to reproduce Reddi et al.'s table.
2. **QFedAvg** — unblocked, exercises ADR 0012's recipe, and addresses a
   tension this project has measured and cannot currently act on.
3. **FedNova** — small server-side, but gated on a client populating
   `local_steps`.
4. **FedProx / SCAFFOLD** — genuinely gated on ADR 0005.

Which means: **two are buildable now, two are waiting on the Python SDK
decision.** That decision is worth making on its own merits, and this is
one more thing that hangs on it.

## What this family does not change

Robustness. An `optimization` member composes *over* an aggregate — it
does not decide which clients are in it. `FedOptAggregator::with_base`
exists so the two can be combined (a Krum pseudo-gradient with adaptive
server optimization on top), and that composition is **not** in Reddi et
al.: it is labelled a deviation wherever it appears, per ADR 0008.
