"""The parts of a PyTorch harness that do not depend on the architecture.

Conflux transmits a flat `f32` vector and knows nothing about the model
behind it. That boundary is what makes this module possible: flattening,
unflattening, local SGD, and evaluation are the same operations whether
the parameters belong to an MLP, a CNN or a GRU, so they are written
once here and every model reuses them.
"""

from __future__ import annotations

import torch
import torch.nn as nn
import torch.nn.functional as F


def param_count(model: nn.Module) -> int:
    """How many weights this model has — the value a deployment must
    give the server as `CONFLUX_INITIAL_WEIGHTS_DIM`."""
    return sum(p.numel() for p in model.parameters())


def flatten(model: nn.Module) -> list[float]:
    """The model's parameters as one flat vector, in `parameters()`
    order — the order every other function here assumes."""
    return torch.cat([p.detach().flatten() for p in model.parameters()]).tolist()


def unflatten(model: nn.Module, flat: list[float]) -> None:
    """Writes a flat vector back into `model`, in place."""
    flat_t = torch.tensor(flat, dtype=torch.float32)
    offset = 0
    for p in model.parameters():
        n = p.numel()
        p.data.copy_(flat_t[offset : offset + n].view_as(p))
        offset += n


def unflatten_like(model: nn.Module, flat: list[float]) -> list[torch.Tensor]:
    """Splits a flat vector into tensors shaped like `model`'s
    parameters, without touching the model. SCAFFOLD needs this: its
    correction and control variates travel flat (the wire format is
    architecture-free) but apply per-parameter during local steps."""
    flat_t = torch.tensor(flat, dtype=torch.float32)
    out, offset = [], 0
    for prm in model.parameters():
        n = prm.numel()
        out.append(flat_t[offset : offset + n].view_as(prm).clone())
        offset += n
    return out


def is_placeholder_init(flat: list[float]) -> bool:
    """True for Conflux's generic all-zero initial checkpoint.

    `conflux-server` has no idea what architecture it is serving, so the
    only initialization it can offer is zeros. That is harmless for a
    model with no hidden layers, and a textbook symmetry-breaking
    failure for anything with them: every hidden unit computes an
    identical zero output with an identical zero gradient, so none ever
    differentiates from the others and the network cannot learn at all,
    no matter how long it trains. A real client recognizes the
    placeholder and substitutes its own deterministic initialization,
    which every client agrees on because the seed is fixed.
    """
    return not any(w != 0.0 for w in flat)


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
    returns its flattened weights — the value a trainer submits.

    `mu > 0` turns this into **FedProx** (Li, Sahu, Zaheer, Sanjabi,
    Talwalkar & Smith, 2018/2020), which minimizes

        h_k(w; w_t) = F_k(w) + (mu/2) * ||w - w_t||^2

    instead of the local loss alone. The extra term penalizes drifting
    away from the round's *starting* weights, which is what stops a
    client with unrepresentative data from running off toward its own
    local optimum during a long round. FedProx is entirely client-side:
    the server sees an ordinary weight vector and cannot tell it was
    used, which is why there is no `aggregator = "fedprox"`.

    `correction` (per-parameter tensors from [`unflatten_like`]) turns
    this into **SCAFFOLD's** client half (Karimireddy et al., 2020,
    Algorithm 1): each step becomes `y <- y - lr * (g - c_i + c)`, so
    pass `correction = c - c_i`. It is added to the *gradient*, not the
    loss — the correction is not the gradient of anything.
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
            proximal = sum(((p - a) ** 2).sum() for p, a in zip(model.parameters(), anchor))
            loss = loss + (mu / 2.0) * proximal
        loss.backward()
        if correction is not None:
            with torch.no_grad():
                for prm, corr in zip(model.parameters(), correction):
                    prm.grad += corr
        opt.step()
    return flatten(model)


def evaluate(model: nn.Module, X: torch.Tensor, y: torch.Tensor) -> tuple[float, float]:
    """Accuracy and cross-entropy loss of `model` on `(X, y)`."""
    model.eval()
    with torch.no_grad():
        out = model(X)
        loss = F.cross_entropy(out, y).item()
        acc = (out.argmax(dim=1) == y).float().mean().item()
    return acc, loss
