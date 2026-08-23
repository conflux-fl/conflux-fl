"""A small real PyTorch MLP for MNIST, plus the flatten/unflatten step
Conflux's wire format needs (unlike Option A's logistic regression,
whose weights already *are* a flat vector) — see docs/E2E_TESTING.md's
"Choosing a model + dataset" section for why this is Option B, not the
starting point.
"""

import torch
import torch.nn as nn
import torch.nn.functional as F

HIDDEN = 64


class MLP(nn.Module):
    def __init__(self):
        super().__init__()
        self.fc1 = nn.Linear(28 * 28, HIDDEN)
        self.fc2 = nn.Linear(HIDDEN, 10)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = x.view(x.size(0), -1)
        x = F.relu(self.fc1(x))
        return self.fc2(x)


def new_model() -> MLP:
    torch.manual_seed(0)  # every client/eval process starts from the same init
    return MLP()


def param_count(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters())


def flatten(model: nn.Module) -> list[float]:
    return torch.cat([p.detach().flatten() for p in model.parameters()]).tolist()


def is_placeholder_init(flat: list[float]) -> bool:
    """True for Conflux's generic all-zero initial checkpoint
    (`conflux-server`'s `main.rs` has no idea what architecture it's
    serving — ADR 0004 — so it can only ever hand out zeros as a
    placeholder). Zero-initializing every weight is fine for a model
    with no hidden layers (Option A's logistic regression), but it's a
    textbook symmetry-breaking failure for a network with ReLU hidden
    units: every unit computes an identical zero output with an
    identical zero gradient, so none of them ever differentiate from
    each other — the network is mathematically incapable of learning
    from that starting point, no matter how many steps you train it
    (see docs/E2E_TESTING.md's "A real finding" section, Option B). A
    real client recognizes this placeholder and substitutes a real,
    architecture-aware initialization instead — `new_model()`'s own
    (deterministic, so every client agrees on the same "shared initial
    model" FL still needs).
    """
    return not any(w != 0.0 for w in flat)


def unflatten(model: nn.Module, flat: list[float]) -> None:
    flat_t = torch.tensor(flat, dtype=torch.float32)
    offset = 0
    for p in model.parameters():
        n = p.numel()
        p.data.copy_(flat_t[offset : offset + n].view_as(p))
        offset += n


def train_steps(
    model: nn.Module,
    X: torch.Tensor,
    y: torch.Tensor,
    lr: float,
    steps: int,
    batch_size: int = 32,
) -> list[float]:
    """Mutates `model` in place via `steps` mini-batch SGD updates, then
    returns its flattened weights — the value a trainer client submits."""
    model.train()
    opt = torch.optim.SGD(model.parameters(), lr=lr)
    n = len(X)
    for _ in range(steps):
        idx = torch.randint(0, n, (min(batch_size, n),))
        opt.zero_grad()
        loss = F.cross_entropy(model(X[idx]), y[idx])
        loss.backward()
        opt.step()
    return flatten(model)


def evaluate(model: nn.Module, X: torch.Tensor, y: torch.Tensor) -> tuple[float, float]:
    model.eval()
    with torch.no_grad():
        out = model(X)
        loss = F.cross_entropy(out, y).item()
        acc = (out.argmax(dim=1) == y).float().mean().item()
    return acc, loss
