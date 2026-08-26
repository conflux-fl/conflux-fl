#!/usr/bin/env python3
"""Downloads real CIFAR-10 (torchvision) and partitions a subsample of
it across N federated clients — same design as the MNIST demo's
`partition_data.py` (identical CLI, identical IID/Dirichlet split
logic), swapped to CIFAR-10's dataset class and normalization stats.

Subsampled, same reasoning as the MNIST demo: keeps wall-clock
reasonable for a CPU-only demo with a handful of local SGD steps per
round; increase --n-train/--n-held-out for a more realistic (slower) run.

Writes, into --out-dir:
  shard_0.pt .. shard_{N-1}.pt   one client's training data each
  held_out.pt                     never partitioned, for eval_client.py
  pooled.pt                       every shard concatenated, for
                                   centralized_baseline.py's comparison
"""

import argparse

import numpy as np
import torch
import torchvision


def iid_split(idx: np.ndarray, n_clients: int, seed: int):
    rng = np.random.default_rng(seed)
    shuffled = rng.permutation(idx)
    return np.array_split(shuffled, n_clients)


def dirichlet_split(idx: np.ndarray, labels: np.ndarray, n_clients: int, alpha: float, seed: int):
    rng = np.random.default_rng(seed)
    client_idx = [[] for _ in range(n_clients)]
    for c in np.unique(labels):
        c_idx = idx[labels == c]
        rng.shuffle(c_idx)
        proportions = rng.dirichlet([alpha] * n_clients)
        splits = (np.cumsum(proportions) * len(c_idx)).astype(int)[:-1]
        for client, part in enumerate(np.split(c_idx, splits)):
            client_idx[client].extend(part.tolist())
    return [np.array(idx, dtype=int) for idx in client_idx]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--n-clients", type=int, default=5)
    parser.add_argument("--n-train", type=int, default=2000, help="subsample size for the federated pool")
    parser.add_argument("--n-held-out", type=int, default=1000)
    parser.add_argument("--split", choices=["iid", "dirichlet"], default="iid")
    parser.add_argument("--dirichlet-alpha", type=float, default=0.5)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--data-root", default="/tmp/conflux_cifar10")
    parser.add_argument("--out-dir", default=".")
    args = parser.parse_args()

    torch.manual_seed(args.seed)
    transform = torchvision.transforms.Compose(
        [
            torchvision.transforms.ToTensor(),
            # Standard CIFAR-10 per-channel mean/std.
            torchvision.transforms.Normalize(
                (0.4914, 0.4822, 0.4465), (0.2470, 0.2435, 0.2616)
            ),
        ]
    )
    train_ds = torchvision.datasets.CIFAR10(
        root=args.data_root, train=True, download=True, transform=transform
    )
    test_ds = torchvision.datasets.CIFAR10(
        root=args.data_root, train=False, download=True, transform=transform
    )

    rng = np.random.default_rng(args.seed)
    train_idx = rng.choice(len(train_ds), size=min(args.n_train, len(train_ds)), replace=False)
    held_idx = rng.choice(len(test_ds), size=min(args.n_held_out, len(test_ds)), replace=False)

    X_train = torch.stack([train_ds[i][0] for i in train_idx])
    y_train = torch.tensor([train_ds[i][1] for i in train_idx])
    X_held = torch.stack([test_ds[i][0] for i in held_idx])
    y_held = torch.tensor([test_ds[i][1] for i in held_idx])

    if args.split == "iid":
        shard_idx = iid_split(np.arange(len(X_train)), args.n_clients, args.seed)
    else:
        shard_idx = dirichlet_split(
            np.arange(len(X_train)), y_train.numpy(), args.n_clients, args.dirichlet_alpha, args.seed
        )

    for i, idx in enumerate(shard_idx):
        path = f"{args.out_dir}/shard_{i}.pt"
        torch.save({"X": X_train[idx], "y": y_train[idx]}, path)
        print(f"wrote {path}: {len(idx)} samples")

    torch.save({"X": X_held, "y": y_held}, f"{args.out_dir}/held_out.pt")
    torch.save({"X": X_train, "y": y_train}, f"{args.out_dir}/pooled.pt")
    print(f"wrote held_out.pt: {len(X_held)} samples")
    print(f"wrote pooled.pt: {len(X_train)} samples (for the centralized baseline)")


if __name__ == "__main__":
    main()
