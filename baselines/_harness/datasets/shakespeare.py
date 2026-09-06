"""Next-character prediction over Shakespeare.

The one dataset here that is non-IID *because of what the data is*
rather than because a partitioner made it so — different plays and
different speakers have genuinely different character statistics. That
makes it the honest test of a robustness or fairness claim.

Falls back to a small embedded excerpt when the corpus cannot be
downloaded, so the harness still runs offline; the fallback is stated in
the output rather than silently substituted.
"""

from __future__ import annotations

import urllib.error
import urllib.request
from pathlib import Path

import numpy as np
import torch

_URL = "https://raw.githubusercontent.com/karpathy/char-rnn/master/data/tinyshakespeare/input.txt"
_SEQ = 40
_FALLBACK = (
    "To be, or not to be, that is the question: "
    "Whether tis nobler in the mind to suffer "
    "The slings and arrows of outrageous fortune, "
    "Or to take arms against a sea of troubles. "
) * 40


def _corpus(root: str) -> str:
    path = Path(root) / "tinyshakespeare.txt"
    if path.exists():
        return path.read_text(encoding="utf-8")
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with urllib.request.urlopen(_URL, timeout=30) as response:
            text = response.read().decode("utf-8")
        path.write_text(text, encoding="utf-8")
        return text
    except (urllib.error.URLError, TimeoutError, OSError) as e:
        print(f"shakespeare: download failed ({e}); using the embedded excerpt")
        return _FALLBACK


def _vocab(text: str) -> dict[str, int]:
    # Fixed at 65 symbols to match the model's embedding table; anything
    # rarer folds into the last index rather than resizing the model.
    chars = sorted(set(text))[:65]
    return {c: i for i, c in enumerate(chars)}


def _windows(text: str, n: int, seed: int):
    vocab = _vocab(text)
    unknown = len(vocab) - 1
    encoded = np.array([vocab.get(c, unknown) for c in text], dtype=np.int64)
    starts = np.arange(0, len(encoded) - _SEQ - 1)
    rng = np.random.default_rng(seed)
    chosen = rng.choice(starts, size=min(n, len(starts)), replace=False)
    X = np.stack([encoded[s : s + _SEQ] for s in chosen])
    y = encoded[chosen + _SEQ]
    return torch.from_numpy(X), torch.from_numpy(y)


def train(n: int, seed: int, root: str):
    return _windows(_corpus(root), n, seed)


def held_out(n: int, seed: int, root: str):
    # A different seed, so the held-out windows are drawn from elsewhere
    # in the corpus than the training windows.
    return _windows(_corpus(root), n, seed + 10_000)
