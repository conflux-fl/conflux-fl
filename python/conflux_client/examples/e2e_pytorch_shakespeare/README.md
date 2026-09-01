# End-to-end demo: Shakespeare + a character-level GRU

Real corpus, real recurrent model, real gRPC, real aggregation —
federated across N clients through Conflux, with **one client per
speaking role**.

## Why this harness exists

The other three harnesses (`e2e_numpy_logreg`, `e2e_pytorch_mnist`,
`e2e_pytorch_cifar10`) are all feed-forward classifiers over
independently-sampled data. Validating a robustness or fairness result
on all three tells you it holds across three *datasets* of one shape. It
does not tell you it holds across a change of *task*.

This one differs on two axes that matter:

1. **A sequence task with a recurrent model.** Next-character prediction
 with a GRU, so gradients flow through time and updates behave
 differently — recurrent nets exploding a gradient looks, to an
 aggregator, a lot like a client attacking it.
2. **Natural non-IID-ness.** Each client is a different Shakespeare
 character. Their vocabulary, cadence, and subject matter genuinely
 differ, so the federation is non-IID *because of what the data is* —
 not because a Dirichlet concentration parameter was set to 0.1. Any
 result that depends on a synthetic skew knob is partly a result about
 the knob; this partition has no knob.

This is the partition LEAF's own Shakespeare benchmark uses, for the
same reason.

## Running it

```bash
# from python/conflux_client/, with the venv active
cd examples/e2e_pytorch_shakespeare

./run_demo.sh # fedavg, 5 clients, 15 rounds, IID control
./run_demo.sh fedavg 5 15 --dirichlet # the by-role (non-IID) partition
./run_demo.sh krum 5 15 --poison --no-reputation # with a Byzantine client
```

`--dirichlet` selects the by-role partition. The flag keeps that name —
rather than something accurate like `--by-role` — so `benchmark.py`'s
`--splits` sweep works unchanged across every harness. A character-level
language model has no class labels to skew, so the natural per-speaker
split is simply what "non-IID" means for this dataset.

First run downloads the corpus (~1 MB) to `/tmp/conflux_shakespeare`;
cached after that.

## What you should see

```
=== 3. centralized baseline (target accuracy) ===
held_out_accuracy=0.2040
round=1 held_out_accuracy=0.0170 held_out_loss=4.1992
round=5 held_out_accuracy=0.1710 held_out_loss=3.0012
```

Chance is 1/65 ≈ 1.5% (the corpus's alphabet size), so ~17% after five
rounds is real learning, converging toward the centralized baseline the
same way the other harnesses do. Accuracy is lower than MNIST's in
absolute terms because next-character prediction is genuinely harder,
not because anything is wrong — compare against the baseline printed in
step 3, never against MNIST's numbers.

The held-out set is drawn from speaking roles **no client trains on**,
so evaluation measures the global model's general Shakespeare rather
than how well it memorized the five speakers in the federation.

## Files

Only two files know what the task is:

| File | Role |
|---|---|
| `model.py` | The GRU, and the flatten/unflatten Conflux's wire format needs |
| `partition_data.py` | Downloads the corpus, splits it by speaking role |

`trainer_client.py`, `eval_client.py`, and `centralized_baseline.py` are
the MNIST harness's, unchanged except for one addition: a **vocabulary
handshake**. Every process reads `vocab.pt` and pins the model's output
width before constructing it, because a character model's final layer is
sized by the alphabet — and two clients that disagreed about it would
build subtly different architectures whose weight vectors happen to be
different lengths, surfacing as a confusing error at the aggregator
rather than an obvious one at startup.

## Known limits

- **One role per client, largest roles first.** The corpus's long tail
 of one-line roles can't train anything, so `--n-clients` is capped by
 how many roles have enough text. This makes the federation less
 heterogeneous than LEAF's full 1,129-client version.
- **Small by design.** `SEQ_LEN=40`, 64 hidden units, 800 samples per
 client — sized so a demo round finishes in seconds on a CPU. LEAF's
 configuration (80 characters of context, a 2-layer 256-unit LSTM) is
 the reference point for a real experiment, not this.
- **Ports are fixed** (50051, 8080, 47100+). Two demos cannot run
 concurrently — the same constraint every harness here has.
