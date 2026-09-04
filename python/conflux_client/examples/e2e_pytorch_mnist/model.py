"""A small real PyTorch MLP for MNIST, plus the flatten/unflatten step
Conflux's wire format needs (unlike Option A's logistic regression,
whose weights already *are* a flat vector) — see the E2E harnesses
guide (https://confluxfl.dev/guides/e2e-harnesses/) for why this is Option B, not the
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
    (`conflux-server` has no idea what architecture it's serving, so it
    can only ever hand out zeros as a
    placeholder). Zero-initializing every weight is fine for a model
    with no hidden layers (Option A's logistic regression), but it's a
    textbook symmetry-breaking failure for a network with ReLU hidden
    units: every unit computes an identical zero output with an
    identical zero gradient, so none of them ever differentiate from
    each other — the network is mathematically incapable of learning
    from that starting point, no matter how many steps you train it
    (see the E2E harnesses guide's "Real findings", Option B). A
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


def unflatten_like(model: nn.Module, flat: list[float]) -> list[torch.Tensor]:
    """Splits a flat vector into tensors shaped like `model`'s parameters,
    without touching the model. SCAFFOLD needs this: its correction and
    control variates travel flat (the wire format is architecture-free)
    but apply per-parameter during local steps."""
    flat_t = torch.tensor(flat, dtype=torch.float32)
    out, offset = [], 0
    for prm in model.parameters():
        n = prm.numel()
        out.append(flat_t[offset : offset + n].view_as(prm).clone())
        offset += n
    return out


def train_steps(
    model: nn.Module,
    X: torch.Tensor,
    y: torch.Tensor,
    lr: float,
    steps: int,
    batch_size: int = 32,
    mu: float = 0.0,
    correction: list[torch.Tensor] | None = None,
) -> list[float]:
    """Mutates `model` in place via `steps` mini-batch SGD updates, then
    returns its flattened weights — the value a trainer client submits.

    `mu > 0` turns this into **FedProx** (Li, Sahu, Zaheer, Sanjabi,
    Talwalkar & Smith, 2018/2020, *Federated Optimization in
    Heterogeneous Networks*), which minimizes

        h_k(w; w_t) = F_k(w) + (mu/2) * ||w - w_t||^2

    instead of the local loss `F_k` alone. The extra term penalizes
    drifting away from the round's *starting* weights, which is what
    stops a client with unrepresentative data from running off toward
    its own local optimum during a long round.

    FedProx is entirely client-side — the server sees an ordinary weight
    vector and cannot tell it was used. That is why there is
    no `aggregator = "fedprox"`: its server half *is* FedAvg, and
    `build_aggregator` says so explicitly if you try.

    `correction` (per-parameter tensors from `unflatten_like`) turns this
    into **SCAFFOLD's** client half (Karimireddy et al., 2020, Algorithm
    1): each step becomes `y <- y - lr * (g - c_i + c)`, so pass
    `correction = c - c_i`. It is added to the *gradient*, not the loss —
    the correction is not the gradient of anything.
    """
    model.train()
    opt = torch.optim.SGD(model.parameters(), lr=lr)
    n = len(X)

    # The anchor is `w_t`, the weights this round *started* from — not
    # the previous local iterate. Snapshotted before the first step and
    # detached, so it is a constant the optimizer never touches.
    anchor = [p.detach().clone() for p in model.parameters()] if mu > 0 else None

    for _ in range(steps):
        idx = torch.randint(0, n, (min(batch_size, n),))
        opt.zero_grad()
        loss = F.cross_entropy(model(X[idx]), y[idx])
        if anchor is not None:
            proximal = sum(
                ((p - a) ** 2).sum() for p, a in zip(model.parameters(), anchor)
            )
            loss = loss + (mu / 2.0) * proximal
        loss.backward()
        if correction is not None:
            with torch.no_grad():
                for prm, corr in zip(model.parameters(), correction):
                    prm.grad += corr
        opt.step()
    return flatten(model)


def evaluate(model: nn.Module, X: torch.Tensor, y: torch.Tensor) -> tuple[float, float]:
    model.eval()
    with torch.no_grad():
        out = model(X)
        loss = F.cross_entropy(out, y).item()
        acc = (out.argmax(dim=1) == y).float().mean().item()
    return acc, loss
