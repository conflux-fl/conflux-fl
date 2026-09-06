"""Finding the `ClientApp` SDK from inside the harness.

The SDK lives in `python/conflux_client`, three directories up and
across. Importing it needs that on `sys.path`, and doing it in one place
means a moved SDK is one edit rather than one per entry point.
"""

from __future__ import annotations

import sys
from pathlib import Path

SDK_DIR = Path(__file__).resolve().parents[2] / "python" / "conflux_client"


def install() -> None:
    """Puts the SDK (and its generated protobuf stubs) on `sys.path`."""
    path = str(SDK_DIR)
    if path not in sys.path:
        sys.path.insert(0, path)
