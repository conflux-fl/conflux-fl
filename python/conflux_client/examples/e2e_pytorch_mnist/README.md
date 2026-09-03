# Option B: PyTorch + real MNIST end-to-end demo

The higher-fidelity version of [Option A](../e2e_numpy_logreg/README.md):
a real PyTorch MLP trained on real MNIST digits, federated across
several simulated clients through the real Conflux pipeline. Read
Option A's README first if you haven't run either demo before — the
two share almost all of their design (same round-polling client loop,
same eval-only client, same centralized-baseline comparison); this one
only differs in the model/dataset and one MNIST-specific gotcha below.

See [`docs/E2E_TESTING.md`](../../../../docs/E2E_TESTING.md) for the
full design rationale.

## Prerequisites

Same as Option A, plus PyTorch/torchvision (a real download, ~200MB —
the CPU build, no GPU needed):

```bash
cd python/conflux_client
python3 -m venv .venv # if you haven't already
.venv/bin/pip install -r requirements.txt
.venv/bin/pip install -r examples/e2e_pytorch_mnist/requirements.txt
./generate_proto.sh
```

The first run also downloads MNIST itself (~10MB) to `/tmp/conflux_mnist`
— cached after that, so only the very first `./run_demo.sh` pays for it.

## Run it

```bash
source .venv/bin/activate
cd examples/e2e_pytorch_mnist
./run_demo.sh # fedavg, 5 clients, 15 rounds
./run_demo.sh krum 5 15 --poison --no-reputation
```

Same script structure as Option A — see that README for what each step
does and the full option list (`AGGREGATOR`, `N_CLIENTS`, `ROUNDS`,
`--poison`, `--no-reputation`), and for the reputation-filtering finding
(applies identically here — it's the same Rust pipeline regardless of
which model the Python side trains).

`krum` really does defend a real neural network, not just Option A's
logistic regression — confirmed directly:

```
./run_demo.sh krum 5 15 --poison --no-reputation
# round=15 held_out_accuracy=0.8840 (centralized baseline: 0.8890)
```

Essentially matching the undefended baseline, with a persistent
large-magnitude attacker present every round.

## What you should see

Real convergence, real MNIST, matching the centralized baseline within
a couple points:

```
=== 3. centralized baseline (target accuracy) ===
held_out_accuracy=0.8890

=== 6. starting 5 trainer clients + 1 eval client ===
round=2 held_out_accuracy=0.7210
round=6 held_out_accuracy=0.8810
round=9 held_out_accuracy=0.8920
round=15 held_out_accuracy=0.9050
```

## A real finding specific to Option B: zero-init breaks a ReLU network

`conflux-server`'s `main.rs` has no idea what model it's serving (Conflux
only ever sees a flat `f32` vector) — its placeholder
initial checkpoint is all zeros (`CONFLUX_INITIAL_WEIGHTS_DIM` zeros).
For Option A's logistic regression (no hidden layer), that's harmless.
For this MLP, it's a textbook symmetry-breaking failure: with every
weight and bias at exactly zero, every hidden unit computes an
*identical* zero output and an *identical* zero gradient through ReLU —
none of them can ever differentiate from each other, so the network is
mathematically incapable of learning from that starting point, no matter
how many steps you train it. The first version of this demo showed
accuracy stuck at ~0.10–0.115 (exactly random-guessing for 10 classes,
loss stuck at exactly `ln(10) = 2.303`) for the entire run — a real bug
caught by actually running the system end-to-end, not something a
smaller-scale test would have surfaced.

The fix lives in `model.py`'s `is_placeholder_init` — every client
(`trainer_client.py`, `eval_client.py`) detects Conflux's all-zero
placeholder and substitutes its own real, architecture-aware
initialization (`new_model()`, PyTorch's default Kaiming-uniform init)
instead of blindly loading zeros. Every client agrees on the same
substituted init because `new_model()` seeds deterministically
(`torch.manual_seed(0)`) — the "everyone starts from the same shared
initial model" FL invariant is preserved, it's just derived client-side
rather than trusted from the server's placeholder. This is the
architecturally correct fix, not a workaround: Conflux's server
genuinely cannot know the right initialization for an arbitrary model
(the server is opaque to model architecture by design), so a real
client has to own this decision.

## Files

Same structure as Option A — `model.py` (the MLP + flatten/unflatten,
since unlike Option A's logistic regression this model's parameters
aren't already a flat vector), `partition_data.py` (downloads + splits
MNIST), `trainer_client.py`, `eval_client.py`, `centralized_baseline.py`,
`run_demo.sh`.

## Troubleshooting

Same as Option A's, plus:
- **First run is slow**: that's the MNIST download, not training — cached
 after the first run.
- **Accuracy stuck near 0.10 with no attack**: if you're running an
 older checkout of this example, you've hit the zero-init bug described
 above — pull the latest `model.py`/`trainer_client.py`/`eval_client.py`.
