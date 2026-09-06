"""Two convolutions and a classifier — the CIFAR-10 shape. Small on
purpose: a harness exists to prove the pipeline converges on a real
model, not to reach a competitive number."""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F


class CNN(nn.Module):
    def __init__(self, channels: int = 3, classes: int = 10) -> None:
        super().__init__()
        self.conv1 = nn.Conv2d(channels, 16, 3, padding=1)
        self.conv2 = nn.Conv2d(16, 32, 3, padding=1)
        # 32x32 halved twice by the pools below.
        self.fc = nn.Linear(32 * 8 * 8, classes)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = F.max_pool2d(F.relu(self.conv1(x)), 2)
        x = F.max_pool2d(F.relu(self.conv2(x)), 2)
        return self.fc(x.flatten(1))


def build() -> CNN:
    torch.manual_seed(0)
    return CNN()
