"""MNIST, normalized with the standard statistics."""

from __future__ import annotations

import numpy as np
import torch

_MEAN, _STD = 0.1307, 0.3081


def _load(train: bool, n: int, seed: int, root: str):
    import torchvision

    transform = torchvision.transforms.Compose(
        [
            torchvision.transforms.ToTensor(),
            torchvision.transforms.Normalize((_MEAN,), (_STD,)),
        ]
    )
    ds = torchvision.datasets.MNIST(root=root, train=train, download=True, transform=transform)
    rng = np.random.default_rng(seed)
    idx = rng.choice(len(ds), size=min(n, len(ds)), replace=False)
    X = torch.stack([ds[i][0] for i in idx])
    y = torch.tensor([ds[i][1] for i in idx])
    return X, y


def train(n: int, seed: int, root: str):
    return _load(True, n, seed, root)


def held_out(n: int, seed: int, root: str):
    # A different `train` flag, so the held-out set is the real test
    # split rather than data any client could have trained on.
    return _load(False, n, seed, root)
