# `_harness/` — the shared training library

Every baseline's Python edge runs on this package. Before it, each
`e2e_*` example carried its own `model.py`, `partition_data.py`,
`trainer_client.py` and `eval_client.py`, and the CIFAR-10 copies were
duplicated from MNIST unchanged — four places for one fix to be applied
three times.

What actually differs between harnesses turns out to be small: the
architecture, how the dataset loads, and how it is split across clients.
Everything else — flattening a model into the wire's `f32` vector,
recognizing the server's all-zero placeholder, running local SGD steps,
scoring a checkpoint — does not depend on the architecture at all, and
now lives once in `torch_model.py`.

## The recipe

A baseline names *what to train*, not *which directory to run*:

```toml
[experiment]
dataset   = "mnist"
model     = "mlp"
partition = "iid"
```

`conflux-baselines` builds the pieces from the registries below and runs
a real federation: a `conflux-server`, one `conflux-node` per client,
one trainer per node, and an evaluator that registers like any other
client and never submits — so the number it reports is the server's
model as a client receives it.

## Layout

```
_harness/
  torch_model.py   flatten/unflatten, local SGD (FedProx, SCAFFOLD), evaluate
  models/          mlp  cnn  gru  logreg
  datasets/        mnist  cifar10  shakespeare
  partition.py     iid  dirichlet  shard
  prepare.py       download, subsample, partition, write shards
  trainer.py       one manifest-driven ClientApp
  evaluator.py     one manifest-driven evaluator
```

Adding a model or a dataset is a file plus a line in the matching
registry; nothing else changes, because nothing else knows which
architecture it was handed.

## Running one directly

The runner does this for you; these are the same commands it issues.

```bash
cd baselines
python -m _harness.prepare --model mlp --dataset mnist --partition iid \
  --clients 5 --out-dir /tmp/work
python -m _harness.trainer --model mlp --shard /tmp/work/shard_0.pt \
  --address 127.0.0.1:47100 --rounds 15
python -m _harness.evaluator --model mlp --held-out /tmp/work/held_out.pt \
  --address 127.0.0.1:47100 --rounds 15
```

PyTorch comes from `python/conflux_client/.venv`, which the runner finds
on its own.
