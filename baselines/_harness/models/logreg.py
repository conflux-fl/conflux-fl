"""Multinomial logistic regression: no hidden layer, so zeros are a
legitimate initialization and this is the one model the server's
placeholder can start on its own."""

from __future__ import annotations

import torch
import torch.nn as nn


class LogisticRegression(nn.Module):
    def __init__(self, inputs: int = 28 * 28, classes: int = 10) -> None:
        super().__init__()
        self.linear = nn.Linear(inputs, classes)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        return self.linear(x.view(x.size(0), -1))


def build() -> LogisticRegression:
    torch.manual_seed(0)
    return LogisticRegression()
