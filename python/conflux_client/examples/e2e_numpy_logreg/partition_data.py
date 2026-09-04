#!/usr/bin/env python3
"""Generates a synthetic binary-classification dataset and partitions it
across N federated clients — see the E2E harnesses guide ("Option A").

Writes, into --out-dir:
  shard_0.npz .. shard_{N-1}.npz   one client's training data each
  held_out.npz                      never partitioned, for eval_client.py
  pooled.npz                        every shard concatenated, for
                                     centralized_baseline.py's comparison

No download, no license concerns, fully reproducible with a fixed seed.
"""

import argparse

import numpy as np
from sklearn.datasets import make_classification
from sklearn.model_selection import train_test_split


def iid_split(X, y, n_clients: int, seed: int):
    rng = np.random.default_rng(seed)
    idx = rng.permutation(len(X))
    return [(X[chunk], y[chunk]) for chunk in np.array_split(idx, n_clients)]


def dirichlet_split(X, y, n_clients: int, alpha: float, seed: int):
    rng = np.random.default_rng(seed)
    classes = np.unique(y)
    client_idx = [[] for _ in range(n_clients)]
    for c in classes:
        c_idx = np.where(y == c)[0]
        rng.shuffle(c_idx)
        proportions = rng.dirichlet([alpha] * n_clients)
        splits = (np.cumsum(proportions) * len(c_idx)).astype(int)[:-1]
        for client, part in enumerate(np.split(c_idx, splits)):
            client_idx[client].extend(part.tolist())
    return [(X[np.array(idx, dtype=int)], y[np.array(idx, dtype=int)]) for idx in client_idx]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--n-clients", type=int, default=5)
    parser.add_argument("--n-samples", type=int, default=2000)
    parser.add_argument("--n-features", type=int, default=10)
    parser.add_argument(
        "--split",
        choices=["iid", "dirichlet"],
        default="iid",
        help="iid: prove the pipeline first. dirichlet: realistic non-IID, once iid works.",
    )
    parser.add_argument(
        "--dirichlet-alpha",
        type=float,
        default=0.5,
        help="smaller = more skewed per-client class distribution",
    )
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--out-dir", default=".")
    args = parser.parse_args()

    X, y = make_classification(
        n_samples=args.n_samples,
        n_features=args.n_features,
        n_informative=max(2, args.n_features - 2),
        n_redundant=0,
        random_state=args.seed,
    )
    X = X.astype(np.float32)
    y = y.astype(np.float32)

    # Held out first, before any partitioning — never seen by a trainer.
    X_train, X_held_out, y_train, y_held_out = train_test_split(
        X, y, test_size=0.2, random_state=args.seed, stratify=y
    )

    if args.split == "iid":
        shards = iid_split(X_train, y_train, args.n_clients, args.seed)
    else:
        shards = dirichlet_split(X_train, y_train, args.n_clients, args.dirichlet_alpha, args.seed)

    for i, (X_i, y_i) in enumerate(shards):
        path = f"{args.out_dir}/shard_{i}.npz"
        np.savez(path, X=X_i, y=y_i)
        print(f"wrote {path}: {len(X_i)} samples, class balance {y_i.mean():.2f}")

    np.savez(f"{args.out_dir}/held_out.npz", X=X_held_out, y=y_held_out)
    np.savez(f"{args.out_dir}/pooled.npz", X=X_train, y=y_train)
    print(f"wrote held_out.npz: {len(X_held_out)} samples")
    print(f"wrote pooled.npz: {len(X_train)} samples (for the centralized baseline)")
    print(f"n_features (== model dimension, minus the bias term): {args.n_features}")


if __name__ == "__main__":
    main()
