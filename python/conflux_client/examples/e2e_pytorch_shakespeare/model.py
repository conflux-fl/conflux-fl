"""A small character-level GRU language model over Shakespeare, plus the
flatten/unflatten step Conflux's wire format needs.

Deliberately a *different shape of problem* from the other harnesses.
`e2e_numpy_logreg`, `e2e_pytorch_mnist`, and `e2e_pytorch_cifar10` are
all i.i.d.-ish image/tabular classification with a feed-forward model. If
every validation runs on that one shape, "these findings generalize" is
an untested claim. This one is sequence modelling with a recurrent net
and a natural, non-synthetic federated partition (by speaking role) — so
a robustness or fairness result that holds here is holding across a real
change of task, not a change of dataset.

Same public API as every other harness's `model.py`, so `trainer_client.py`,
`eval_client.py`, and `centralized_baseline.py` are shared unchanged: only
the model and the partitioning differ.
"""

import torch
import torch.nn as nn
import torch.nn.functional as F

# The vocabulary is derived from the corpus, but every process
# (partitioner, trainers, evaluator, baseline) has to agree on it exactly
# or the models are not the same model. Sorting the unique characters of
# the full text is the cheapest way to make that agreement deterministic
# without shipping a vocabulary file alongside the shards.
SEQ_LEN = 40
EMBED = 8
HIDDEN = 64


def build_vocab(text: str) -> list[str]:
    return sorted(set(text))


class CharGRU(nn.Module):
    def __init__(self, vocab_size: int):
        super().__init__()
        self.embed = nn.Embedding(vocab_size, EMBED)
        self.gru = nn.GRU(EMBED, HIDDEN, batch_first=True)
        self.out = nn.Linear(HIDDEN, vocab_size)

    def forward(self, x: torch.Tensor) -> torch.Tensor:
        # x: [batch, SEQ_LEN] of character indices -> logits for the
        # single next character, i.e. the last timestep's hidden state.
        h = self.embed(x)
        _, last = self.gru(h)
        return self.out(last.squeeze(0))


# Fixed at import time from the corpus's own alphabet. `partition_data.py`
# writes it next to the shards so every process reads the same one rather
# than re-deriving it from whatever text it happens to hold.
VOCAB_SIZE = 65


def set_vocab_size(size: int) -> None:
    """Called by any process that has read `vocab.pt`, before
    `new_model()`. Kept explicit rather than auto-loading a file, so a
    process that forgets fails loudly on a shape mismatch instead of
    silently training a differently-shaped model."""
    global VOCAB_SIZE
    VOCAB_SIZE = size


def new_model() -> CharGRU:
    torch.manual_seed(0)  # every client/eval process starts from the same init
    return CharGRU(VOCAB_SIZE)


def param_count(model: nn.Module) -> int:
    return sum(p.numel() for p in model.parameters())


def flatten(model: nn.Module) -> list[float]:
    return torch.cat([p.detach().flatten() for p in model.parameters()]).tolist()


def is_placeholder_init(flat: list[float]) -> bool:
    """True for Conflux's generic all-zero initial checkpoint.

    Same reasoning as the MNIST harness's version, and it matters more
    here: a GRU started from all zeros has zero gates as well as zero
    weights, so it cannot break symmetry *or* propagate a hidden state.
    `conflux-server` has no idea what architecture it is serving and can
    only hand out zeros, so a real client recognizes the
    placeholder and substitutes its own deterministic initialization.
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
        # Recurrent nets are prone to exploding gradients, and an
        # exploding update from one client would look exactly like an
        # attack to the aggregator being tested. Clipping here keeps the
        # experiment measuring robustness rather than optimizer noise.
        torch.nn.utils.clip_grad_norm_(model.parameters(), 5.0)
        opt.step()
    return flatten(model)


def evaluate(model: nn.Module, X: torch.Tensor, y: torch.Tensor) -> tuple[float, float]:
    """Next-character accuracy and cross-entropy. Chance is 1/vocab_size
    (~1.5%), so accuracy is a meaningful signal here even though the task
    is generative rather than classification."""
    model.eval()
    with torch.no_grad():
        out = model(X)
        loss = F.cross_entropy(out, y).item()
        acc = (out.argmax(dim=1) == y).float().mean().item()
    return acc, loss
