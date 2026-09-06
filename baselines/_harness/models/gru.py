"""A character-level GRU — the Shakespeare shape, and the one model here
with recurrent state.

A GRU started from all zeros has zero gates as well as zero weights, so
it cannot break symmetry *or* propagate a hidden state: the placeholder
problem in its most complete form."""

from __future__ import annotations

import torch
import torch.nn as nn

VOCAB = 65
EMBED = 8
HIDDEN = 64


class CharGRU(nn.Module):
    def __init__(self, vocab: int = VOCAB) -> None:
        super().__init__()
        self.embed = nn.Embedding(vocab, EMBED)
        self.gru = nn.GRU(EMBED, HIDDEN, batch_first=True)
        self.fc = nn.Linear(HIDDEN, vocab)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        out, _ = self.gru(self.embed(x))
        # Next-character prediction: only the last step's state matters.
        return self.fc(out[:, -1, :])


def build() -> CharGRU:
    torch.manual_seed(0)
    return CharGRU()
