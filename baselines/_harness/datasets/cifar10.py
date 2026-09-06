"""CIFAR-10, normalized with the standard per-channel statistics."""

from __future__ import annotations

import numpy as np
import torch

_MEAN = (0.4914, 0.4822, 0.4465)
_STD = (0.2470, 0.2435, 0.2616)


def _load(train: bool, n: int, seed: int, root: str):
    import torchvision

    transform = torchvision.transforms.Compose(
        [
            torchvision.transforms.ToTensor(),
            torchvision.transforms.Normalize(_MEAN, _STD),
        ]
    )
    ds = torchvision.datasets.CIFAR10(root=root, train=train, download=True, transform=transform)
    rng = np.random.default_rng(seed)
    idx = rng.choice(len(ds), size=min(n, len(ds)), replace=False)
    X = torch.stack([ds[i][0] for i in idx])
    y = torch.tensor([ds[i][1] for i in idx])
    return X, y


def train(n: int, seed: int, root: str):
    return _load(True, n, seed, root)


def held_out(n: int, seed: int, root: str):
    return _load(False, n, seed, root)
