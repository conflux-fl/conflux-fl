# Phase 23 — the `ClientApp` SDK (ADR 0005, question 3)

**Status: shipped (2026-09-01).** `python/conflux_client/app.py`, plus
`crates/conflux-client` — the Rust-native second path, built as a spike
and now working end to end. This brief records what was decided, what
was deliberately left undecided, and what the Rust spike actually
measured.

## What ADR 0005 actually deferred

The ADR separates three questions that "the SDK" usually conflates:

1. **How does a client learn what model to train?**
2. **How does a participant *get* the client code?**
3. **What does the SDK wrap?**

Its own recommendation is to resolve **(3) first**, because it is pure
technical design answerable from this codebase alone, and it unblocks
`cross_silo` — where (1) and (2) can stay "assume out-of-band
installation" indefinitely, since that is already how all four e2e
harnesses work.

This phase does exactly that and nothing more. **(1) and (2) remain
deferred**, and the Rust section below is the reason to keep (2)
deferred deliberately rather than answer it in a hurry.

## What shipped

```python
class MnistClient(ClientApp):
    def train(self, weights, round):
        if not is_placeholder_init(weights):
            unflatten(self.model, weights)
        trained = train_steps(self.model, self.X, self.y, self.lr, self.steps)
        return TrainResult(weights=trained, num_samples=len(self.y),
                           local_steps=self.steps, local_loss=loss_before)
```

The base class owns everything that was previously copy-pasted into
every client: connect, register, the fetch-until-a-new-round loop,
placeholder-init detection, f32-aligned chunking, submit-with-retry, and
treating a round that closed mid-training as ordinary rather than fatal.

**Four separate copies of a `struct.pack`/`unpack` codec** existed across
the harnesses and the stub. There is now one.

### The part that unblocks four methods

`TrainResult` carries `local_steps`, `local_loss` and `control_variate`.
Those wire fields have existed since ADR 0012 and **nothing has ever
been able to populate them** — which is precisely why FedNova, SCAFFOLD
and q-FedAvg are shipped-but-inert. This is the piece that changes that.

The migrated MNIST harness now reports `local_steps` and `local_loss`,
making it the first client in the project's history to send either.

## Two things found while building it

**The generated Python stubs were stale.** `fl_transport_pb2.py` predated
ADR 0012 entirely — no `local_steps`, no `control_variate`, no
`local_loss`. The Python side had been silently out of sync with the
schema since those fields landed, and nothing would have noticed, because
generated files are not committed and no test imports them. Regenerating
is one command; *knowing to* was the problem. Worth a CI step.

**`absent` and `zero` are the same value in Python.** protobuf reads an
unset `optional float` as `0.0`, so a client checking truthiness cannot
distinguish "not running q-FedAvg" from "reported a loss of exactly
zero" — and under `q > 0` a loss read as zero means *zero weight*,
silently excluding every client not yet upgraded. `HasField` is the only
correct check. Pinned as tests on both sides of the wire.

## The Rust alternative, built

A Rust-native client — training in `conflux-node` via
[Burn](https://github.com/tracel-ai/burn) — would collapse the local
loopback hop entirely and **change what questions (1) and (2) even
mean**:

| ADR 0005 asks | Python answer | Rust answer |
|---|---|---|
| (1) model architecture handoff | import path, or ship a serialized model over a new proto field | **compiled in** — no handoff exists |
| (2) client code distribution | pip / container / web push — the ADR calls this "the harder question... outside this codebase's boundary" | **one static binary** |
| (3) what the SDK wraps | this phase | a Rust trait |

Question (2) is the one ADR 0005 says it cannot resolve unilaterally.
For `crowdsource` and `edge`, "ship a binary" is a dramatically smaller
problem than provisioning a Python environment on machines nobody
controls. It would also make the `edge` topology real — its profile
currently just mirrors `cross_device`, with the code admitting that
resource-aware tuning "isn't implemented yet", and Burn's `no_std`
backend targets exactly that.

**Xaynet is the obvious precedent and it does not apply.** It was
Rust-native FL and it is archived (2022) — but it never trained in Rust.
It was model-agnostic masking and aggregation with Dart/Flutter client
SDKs, so its failure says nothing about whether Rust-side *training*
works. Worth knowing before citing it in either direction.

### The spike, and what it found

The cheap test this brief proposed — a Rust equivalent of
`e2e_numpy_logreg` — was built: `crates/conflux-client`, the fifteenth
crate. It is the same contract as the Python SDK, field for field, so a
divergence between the two is a bug in one of them rather than a design
choice:

```rust
impl ClientApp for LogRegClient {
    fn train(&mut self, weights: &[f32], round: u64) -> TrainResult {
        let trained = train_steps(weights, &self.xs, &self.ys, self.lr, self.steps);
        TrainResult::new(trained, self.ys.len() as u64)
            .with_local_steps(self.steps as u32)
            .with_local_loss(loss_before)
    }
}
```

It needed **no new proto field, no server change, and no `conflux-node`
change**. `PullTransport` already *was* the client half of the local
hop, because ADR 0004 made both hops speak one schema — so the Rust
client reuses the transport the Python client talks to, rather than
paralleling it. That is the architectural result, and it is the one
worth keeping: the loopback hop is a language boundary, not a design
seam, and removing it removes a process rather than a layer.

**The measured run** — real `conflux-server`, four real `conflux-node`s,
four Rust clients, eight rounds, no Python process anywhere:

```
rc-0 (sees feature 0): local-only 0.682 -> federated 0.996
rc-1 (sees feature 1): local-only 0.666 -> federated 0.996
rc-2 (sees feature 2): local-only 0.682 -> federated 0.996
rc-3 (sees feature 3): local-only 0.676 -> federated 0.996
```

Round 2 scored 0.986, round 3 0.994, rounds 4–8 0.996.

The `local-only` column is the part that makes this evidence. The first
version of the example sharded IID and every client hit 1.000 on its own
data *before* federating — which proved the loop ran and nothing else.
The shipped version gives client *i* data where only feature *i* varies,
against a label of `sum(x) > 0`, so **no client can solve the problem
alone** and all four are scored on the same held-out global test set.

It also reports `local_steps` and `local_loss`, making it the second
client able to populate ADR 0012's fields at all.

### What the spike deliberately does not answer

**Which ML framework.** Logistic regression needs none — the gradient is
a few lines — which is exactly why it is the right spike: it isolates
the *architecture* from the *ML stack*. Anything with hidden layers
wants [Burn](https://github.com/tracel-ai/burn), and that is a separate
evaluation with a separate cost:

- **Burn is pre-1.0 and says so.** This project spent six tiers reaching
  stability and published an API-stability policy; taking a dependency
  that advertises breaking changes cuts against that. Confining it to a
  separate optional crate — the `conflux-trusted-reference` pattern —
  would contain the blast radius.
- **The ecosystem gap is real.** No torchvision equivalent, limited ONNX
  operator coverage, fewer pretrained models. Fine at the linear/MLP/
  small-CNN scale the harnesses use; not fine for a pretrained backbone.

**Whether Rust should replace Python. It should not.** Researchers want
PyTorch, all four e2e harnesses are PyTorch, and the DSS research line
runs on them. This is a *second* path, permanently — more surface, not
less. What it buys is question (2): for `crowdsource` and `edge`,
shipping one static binary is a categorically smaller problem than
provisioning a Python environment on machines nobody controls.

## What remains deferred

- **(1) model architecture handoff** — the ADR recommends a config-borne
  Python import path; nothing here presumes or blocks it.
- **(2) client code distribution** — still a product decision, but no
  longer one being made on incomplete information: the Rust option is
  now measured rather than hypothesized, and "one static binary" is a
  demonstrated option for `crowdsource`/`edge` rather than a claim.
- **Migrating the other three harnesses.** MNIST is migrated and
  verified end to end (0.142 → 0.858 held-out accuracy against a 0.750
  centralized baseline, real gRPC, real training). CIFAR-10, Shakespeare
  and numpy-logreg still carry their own copies of the loop.
- **A CI step that regenerates the Python stubs and fails on drift.**
  The staleness above was silent and will recur.
