"""One hidden layer, ReLU — the smallest network whose hidden units make
the server's all-zero placeholder a real problem, which is why it is the
harness's default and the shape most baselines use."""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F

HIDDEN = 64


class MLP(nn.Module):
    def __init__(self, inputs: int = 28 * 28, classes: int = 10) -> None:
        super().__init__()
        self.fc1 = nn.Linear(inputs, HIDDEN)
        self.fc2 = nn.Linear(HIDDEN, classes)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = x.view(x.size(0), -1)
        x = F.relu(self.fc1(x))
        return self.fc2(x)


def build() -> MLP:
    # Fixed, so every client and the evaluator agree on the starting
    # model without the server having to supply it.
    torch.manual_seed(0)
    return MLP()
