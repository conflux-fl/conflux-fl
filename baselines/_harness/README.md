# `_harness/` — shared training library (Phase 4, not yet populated)

Today every baseline points `[harness].example` at an existing
`python/conflux_client/examples/e2e_*` directory, and the runner drives
that example's proven `run_demo.sh`. That works now and reuses code that
already reproduces real numbers.

**Phase 4** (see `https://confluxfl.dev/guides/baselines/`) refactors the
duplicated `e2e_*` code — the CIFAR trainer/eval/centralized files are
literally "copied over unchanged from MNIST" — into one reusable library
here:

```
_harness/
  models/      mlp.py  cnn.py  gru.py  logreg.py
  datasets/    mnist.py  cifar10.py  shakespeare.py  femnist.py
  partition/   iid.py  dirichlet.py  shard.py
  trainer.py   # one manifest-driven ClientApp
  eval.py      # one manifest-driven evaluator
```

A baseline's `[harness]` then names a recipe
(`model = "mlp"`, `dataset = "mnist"`, `partition = "iid"`) instead of an
example dir, and the Rust runner orchestrates the federation itself
(typed) instead of shelling out to bash. Until then, this directory is a
placeholder marking the intended home.
