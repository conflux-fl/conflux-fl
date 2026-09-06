"""Architectures, by name.

Each entry is a zero-argument builder returning an `nn.Module` with a
*fixed* seed, so every client and the evaluator start from the same
weights — federated learning still needs one shared initial model, and
the server's all-zero placeholder cannot supply it.

Adding one is a file plus a line in [`BUILDERS`]. Everything else a
harness needs comes from `torch_model`, which does not know or care
which architecture it was handed.
"""

from __future__ import annotations

from collections.abc import Callable

import torch.nn as nn

from . import cnn, gru, logreg, mlp

BUILDERS: dict[str, Callable[[], nn.Module]] = {
    "mlp": mlp.build,
    "cnn": cnn.build,
    "gru": gru.build,
    "logreg": logreg.build,
}


def build(name: str) -> nn.Module:
    """The model a recipe names, or a `KeyError` listing what exists."""
    try:
        return BUILDERS[name]()
    except KeyError:
        raise KeyError(
            f"no model named {name!r} (known: {', '.join(sorted(BUILDERS))})"
        ) from None
