#!/usr/bin/env python3
"""Sweeps an e2e demo directory (e2e_pytorch_mnist, e2e_pytorch_cifar10,
...) across a grid of (aggregator, data split, attack) combinations,
capturing every round's real held-out accuracy/loss — plus the
centralized baseline each combination is compared against — into one
JSONL file.

A real-data sweep: each combination is one real `./run_demo.sh` invocation
(real training, real gRPC, real aggregation), not a synthetic vector.

Usage:
  python3 benchmark.py --example-dir e2e_pytorch_mnist \\
      --aggregators fedavg krum multi_krum trimmed_mean median \\
      --splits iid dirichlet:0.5 dirichlet:0.1 \\
      --attacks none poison \\
      --n-clients 5 --rounds 15 \\
      --out results_mnist.jsonl

Each `--splits` entry is either "iid" or "dirichlet:<alpha>" (lower
alpha = more non-IID).

Each `--attacks` entry is "none" or "poison". `poison` turns on the
demo's own persistent Byzantine client (`run_demo.sh --poison`), which
is what makes this harness able to check whether the synthetic-vector
robustness findings from synthetic experiments
survive contact with a real model and a real dataset. Without it a sweep
can only compare aggregators on clean data, where they are all supposed
to look alike — and mostly do, which tells you nothing about
robustness.

Re-runnable: appends to `--out` rather than overwriting, so a sweep can
be extended across multiple invocations — delete the file first for a
clean run.
"""

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

BASELINE_RE = re.compile(r"held_out_accuracy=([\d.]+)")
ROUND_RE = re.compile(r"round=(\d+) held_out_accuracy=([\d.]+) held_out_loss=([\d.]+)")


def parse_split(spec: str) -> tuple[str, float | None]:
    if spec == "iid":
        return "iid", None
    if spec.startswith("dirichlet:"):
        return "dirichlet", float(spec.split(":", 1)[1])
    raise ValueError(f"unrecognized --splits entry {spec!r} (expected 'iid' or 'dirichlet:<alpha>')")


def run_one(
    example_dir: Path,
    aggregator: str,
    split: str,
    dirichlet_alpha: float | None,
    n_clients: int,
    rounds: int,
    attack: str,
) -> tuple[float, list[tuple[int, float, float]]]:
    cmd = [str(example_dir / "run_demo.sh"), aggregator, str(n_clients), str(rounds)]
    if split == "dirichlet":
        cmd += ["--dirichlet", "--dirichlet-alpha", str(dirichlet_alpha)]
    if attack == "poison":
        # Also disables the reputation pre-filter, so what is being
        # measured is the *aggregator's* own robustness rather than
        # whether a separate filter caught the attacker first. That
        # separation is the whole point of the comparison — see
        # docs/E2E_TESTING.md's "Real findings" on the reputation filter
        # masking aggregator differences.
        cmd += ["--poison", "--no-reputation"]

    print(f"  $ {' '.join(cmd)}", file=sys.stderr)
    start = time.time()
    proc = subprocess.run(cmd, cwd=example_dir, capture_output=True, text=True, timeout=1800)
    elapsed = time.time() - start
    output = proc.stdout + proc.stderr

    if proc.returncode != 0:
        print(output[-4000:], file=sys.stderr)
        raise RuntimeError(f"run_demo.sh exited {proc.returncode} after {elapsed:.0f}s — see output above")

    # Step 3's single baseline accuracy line — everything between the
    # "=== 3." and "=== 4." section headers.
    baseline_section = output.split("=== 3. centralized baseline")[1].split("=== 4.")[0]
    baseline_match = BASELINE_RE.search(baseline_section)
    if not baseline_match:
        raise RuntimeError("could not find centralized baseline accuracy in output")
    baseline_acc = float(baseline_match.group(1))

    rounds_seen: dict[int, tuple[float, float]] = {}
    for m in ROUND_RE.finditer(output):
        r, acc, loss = int(m.group(1)), float(m.group(2)), float(m.group(3))
        rounds_seen[r] = (acc, loss)  # last occurrence per round wins

    print(f"    done in {elapsed:.0f}s, {len(rounds_seen)} rounds, baseline={baseline_acc:.4f}", file=sys.stderr)
    return baseline_acc, sorted((r, acc, loss) for r, (acc, loss) in rounds_seen.items())


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--example-dir", required=True, help="e.g. e2e_pytorch_mnist")
    parser.add_argument("--aggregators", nargs="+", required=True)
    parser.add_argument("--splits", nargs="+", default=["iid"])
    parser.add_argument(
        "--attacks",
        nargs="+",
        default=["none"],
        choices=["none", "poison"],
        help="'poison' enables the demo's persistent Byzantine client (and disables the "
        "reputation pre-filter, so the aggregator is what is being tested)",
    )
    parser.add_argument("--n-clients", type=int, default=5)
    parser.add_argument("--rounds", type=int, default=15)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    example_dir = Path(args.example_dir).resolve()
    if not (example_dir / "run_demo.sh").exists():
        print(f"no run_demo.sh in {example_dir}", file=sys.stderr)
        sys.exit(1)

    dataset = example_dir.name.replace("e2e_pytorch_", "").replace("e2e_", "")
    out_path = Path(args.out)

    total = len(args.aggregators) * len(args.splits) * len(args.attacks)
    done = 0
    with out_path.open("a") as f:
        for attack in args.attacks:
            for split_spec in args.splits:
                split, alpha = parse_split(split_spec)
                for aggregator in args.aggregators:
                    done += 1
                    print(
                        f"[{done}/{total}] dataset={dataset} aggregator={aggregator} "
                        f"split={split_spec} attack={attack}",
                        file=sys.stderr,
                    )
                    baseline_acc, rounds = run_one(
                        example_dir, aggregator, split, alpha, args.n_clients, args.rounds, attack
                    )
                    for r, acc, loss in rounds:
                        f.write(
                            json.dumps(
                                {
                                    "dataset": dataset,
                                    "aggregator": aggregator,
                                    "split": split,
                                    "dirichlet_alpha": alpha,
                                    "attack": attack,
                                    "n_clients": args.n_clients,
                                    "round": r,
                                    "held_out_accuracy": acc,
                                    "held_out_loss": loss,
                                    "centralized_baseline_accuracy": baseline_acc,
                                }
                            )
                            + "\n"
                        )
                    f.flush()

    print(f"wrote results to {out_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
