"""A small real PyTorch CNN for CIFAR-10 — same shape of harness as
Option B's MNIST demo (`e2e_pytorch_mnist/model.py`), just a
convolutional model instead of an MLP, since CIFAR-10's 32x32x3 images
need spatial structure an MLP alone won't pick up cheaply. Every helper
here matches that file's own function signatures exactly (`flatten`,
`unflatten`, `is_placeholder_init`, `train_steps`, `evaluate`,
`param_count`) — `trainer_client.py`/`eval_client.py`/
`centralized_baseline.py` are copied over unchanged from the MNIST
demo; only this file and `partition_data.py` are dataset-specific.
"""

import torch
import torch.nn as nn
import torch.nn.functional as F

# Deliberately small — this harness favors a demo that finishes in a
# reasonable wall-clock time on CPU over state-of-the-art CIFAR-10
# accuracy. Two conv blocks is enough to clearly beat random guessing
# (10%) and show real separation between aggregation methods; it is not
# meant to be a competitive CIFAR-10 result on its own.
class SmallCNN(nn.Module):
    def __init__(self):
        super().__init__()
        self.conv1 = nn.Conv2d(3, 16, kernel_size=3, padding=1)
        self.conv2 = nn.Conv2d(16, 32, kernel_size=3, padding=1)
        self.pool = nn.MaxPool2d(2, 2)
        self.fc1 = nn.Linear(32 * 8 * 8, 64)
        self.fc2 = nn.Linear(64, 10)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        x = self.pool(F.relu(self.conv1(x)))  # 32x32 -> 16x16
        x = self.pool(F.relu(self.conv2(x)))  # 16x16 -> 8x8
        x = x.view(x.size(0), -1)
        x = F.relu(self.fc1(x))
        return self.fc2(x)


def new_model() -> SmallCNN:
    torch.manual_seed(0)  # every client/eval process starts from the same init
    return SmallCNN()


def param_count(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters())


def flatten(model: nn.Module) -> list[float]:
    return torch.cat([p.detach().flatten() for p in model.parameters()]).tolist()


def is_placeholder_init(flat: list[float]) -> bool:
    """Same reasoning as the MNIST demo's `model.py` — Conflux's
    all-zero placeholder checkpoint would break a ReLU network's
    symmetry the same way here; every client substitutes its own real,
    architecture-aware init (`new_model()`, deterministic) instead."""
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
    model.train()
    opt = torch.optim.SGD(model.parameters(), lr=lr, momentum=0.9)
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
