"""Splitting a dataset across clients.

The partition is what makes a federation *federated*: an IID split is
the easy case every method handles, and the non-IID ones are where
methods start to differ from each other. A baseline names which one it
reproduces under, because a number without its partition is not a
result.
"""

from __future__ import annotations

import numpy as np


def iid(indices: np.ndarray, n_clients: int, seed: int, **_: object) -> list[np.ndarray]:
    """Shuffle and deal. Every client sees the same distribution, which
    is the assumption most convergence proofs make and most real
    deployments break."""
    rng = np.random.default_rng(seed)
    return list(np.array_split(rng.permutation(indices), n_clients))


def dirichlet(
    indices: np.ndarray,
    n_clients: int,
    seed: int,
    labels: np.ndarray | None = None,
    alpha: float = 0.5,
    **_: object,
) -> list[np.ndarray]:
    """Label skew drawn from a Dirichlet, the standard non-IID benchmark
    (Hsu, Qi & Brown, 2019). Smaller `alpha` is more skewed; at `alpha →
    0` each client holds a single class."""
    if labels is None:
        raise ValueError("the dirichlet partition needs labels")
    rng = np.random.default_rng(seed)
    per_client: list[list[int]] = [[] for _ in range(n_clients)]
    for c in np.unique(labels):
        c_idx = indices[labels == c]
        rng.shuffle(c_idx)
        proportions = rng.dirichlet([alpha] * n_clients)
        splits = (np.cumsum(proportions) * len(c_idx)).astype(int)[:-1]
        for client, part in enumerate(np.split(c_idx, splits)):
            per_client[client].extend(part.tolist())
    return [np.array(idx, dtype=int) for idx in per_client]


def shard(
    indices: np.ndarray,
    n_clients: int,
    seed: int,
    labels: np.ndarray | None = None,
    shards_per_client: int = 2,
    **_: object,
) -> list[np.ndarray]:
    """Sort by label, cut into equal shards, deal `shards_per_client` to
    each — the pathological split from McMahan et al. (2017), where a
    client sees at most two classes. Harsher than a Dirichlet draw and
    exactly reproducible, which is why the original FedAvg paper used
    it."""
    if labels is None:
        raise ValueError("the shard partition needs labels")
    rng = np.random.default_rng(seed)
    order = indices[np.argsort(labels, kind="stable")]
    n_shards = n_clients * shards_per_client
    shards = np.array_split(order, n_shards)
    assignment = rng.permutation(n_shards).reshape(n_clients, shards_per_client)
    return [np.concatenate([shards[s] for s in row]) for row in assignment]


PARTITIONS = {"iid": iid, "dirichlet": dirichlet, "shard": shard}


def split(name: str, **kwargs: object) -> list[np.ndarray]:
    """The partition a recipe names, or a `KeyError` listing what exists."""
    try:
        fn = PARTITIONS[name]
    except KeyError:
        raise KeyError(
            f"no partition named {name!r} (known: {', '.join(sorted(PARTITIONS))})"
        ) from None
    return fn(**kwargs)  # type: ignore[arg-type]
