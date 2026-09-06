"""Datasets, by name.

A loader returns `(X, y)` tensors already normalized and subsampled —
never a `Dataset` object — because everything downstream (partitioning,
shard files, evaluation) works on tensors, and materializing once keeps
the per-client shards cheap to write and cheap to load.

Subsampling is deliberate and is stated in every result: a harness
exists to prove the pipeline converges on real data, not to reach a
competitive number, and a full training set turns a two-minute check
into an afternoon.
"""

from __future__ import annotations

from collections.abc import Callable

import torch

from . import cifar10, mnist, shakespeare

Loader = Callable[..., tuple[torch.Tensor, torch.Tensor]]

LOADERS: dict[str, tuple[Loader, Loader]] = {
    # (train, held-out)
    "mnist": (mnist.train, mnist.held_out),
    "cifar10": (cifar10.train, cifar10.held_out),
    "shakespeare": (shakespeare.train, shakespeare.held_out),
}


def load(name: str, *, n_train: int, n_held_out: int, seed: int, root: str):
    """`((X_train, y_train), (X_held, y_held))` for the named dataset."""
    try:
        train_fn, held_fn = LOADERS[name]
    except KeyError:
        raise KeyError(
            f"no dataset named {name!r} (known: {', '.join(sorted(LOADERS))})"
        ) from None
    return (
        train_fn(n=n_train, seed=seed, root=root),
        held_fn(n=n_held_out, seed=seed, root=root),
    )
