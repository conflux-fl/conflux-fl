#!/usr/bin/env python3
"""Materializes a recipe's data: download, subsample, partition, write.

Writes into `--out-dir`:

    shard_0.pt .. shard_{N-1}.pt   one client's training data each
    held_out.pt                    never partitioned, for the evaluator
    pooled.pt                      every shard, for a centralized baseline

and prints the model's parameter count, which is what a deployment must
hand the server as `CONFLUX_INITIAL_WEIGHTS_DIM`. Printing it rather
than hardcoding it is what keeps a model change from silently becoming
a length error in round one.
"""

from __future__ import annotations

import argparse
import json

import numpy as np
import torch

from . import datasets, models, partition, torch_model


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True)
    parser.add_argument("--dataset", required=True)
    parser.add_argument("--partition", default="iid")
    parser.add_argument("--clients", type=int, default=5)
    parser.add_argument("--n-train", type=int, default=3000)
    parser.add_argument("--n-held-out", type=int, default=1000)
    parser.add_argument("--dirichlet-alpha", type=float, default=0.5)
    parser.add_argument("--shards-per-client", type=int, default=2)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--data-root", default="/tmp/conflux_data")
    parser.add_argument("--out-dir", default=".")
    args = parser.parse_args()

    torch.manual_seed(args.seed)
    (X, y), (X_held, y_held) = datasets.load(
        args.dataset,
        n_train=args.n_train,
        n_held_out=args.n_held_out,
        seed=args.seed,
        root=args.data_root,
    )

    parts = partition.split(
        args.partition,
        indices=np.arange(len(X)),
        n_clients=args.clients,
        seed=args.seed,
        labels=y.numpy(),
        alpha=args.dirichlet_alpha,
        shards_per_client=args.shards_per_client,
    )

    for i, idx in enumerate(parts):
        path = f"{args.out_dir}/shard_{i}.pt"
        torch.save({"X": X[idx], "y": y[idx]}, path)
        print(f"wrote {path}: {len(idx)} samples")
    torch.save({"X": X_held, "y": y_held}, f"{args.out_dir}/held_out.pt")
    torch.save({"X": X, "y": y}, f"{args.out_dir}/pooled.pt")
    print(f"wrote held_out.pt: {len(X_held)} samples")
    print(f"wrote pooled.pt: {len(X)} samples")

    dim = torch_model.param_count(models.build(args.model))
    # A machine-readable line the runner reads, beside the human ones.
    print(json.dumps({"model_dim": dim, "clients": len(parts)}))


if __name__ == "__main__":
    main()
