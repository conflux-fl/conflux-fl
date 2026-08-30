#!/usr/bin/env python3
"""Converts benchmark.py's JSONL output into a flat CSV (every round)
and a summary CSV (final-round accuracy per aggregator x split) — no
dependencies beyond the standard library, matching
docs/research/scripts/summarize.py's own convention for the Rust-side
experiments.

Usage:
    python3 summarize_benchmark.py results.jsonl
"""

import csv
import json
import sys
from collections import defaultdict
from pathlib import Path


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <results.jsonl>", file=sys.stderr)
        sys.exit(1)

    in_path = Path(sys.argv[1])
    rows = [json.loads(line) for line in in_path.read_text().splitlines() if line.strip()]
    if not rows:
        print(f"no rows in {in_path}", file=sys.stderr)
        sys.exit(1)

    full_csv = in_path.with_suffix(".csv")
    fieldnames = list(rows[0].keys())
    with full_csv.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)
    print(f"wrote {len(rows)} rows -> {full_csv}")

    groups: dict[tuple, list[dict]] = defaultdict(list)
    for r in rows:
        # `attack` defaults for rows written before benchmark.py grew
        # that dimension, so older results files still summarize.
        groups[
            (r["dataset"], r["aggregator"], r["split"], r["dirichlet_alpha"], r.get("attack", "none"))
        ].append(r)

    summary_csv = in_path.with_name(in_path.stem + ".summary.csv")
    with summary_csv.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(
            [
                "dataset",
                "aggregator",
                "split",
                "dirichlet_alpha",
                "attack",
                "final_round",
                "final_held_out_accuracy",
                "centralized_baseline_accuracy",
                "accuracy_gap_vs_baseline",
            ]
        )
        for (dataset, aggregator, split, alpha, attack), group in sorted(
            groups.items(), key=lambda kv: (kv[0][0], kv[0][4], kv[0][2], kv[0][1])
        ):
            last = max(group, key=lambda r: r["round"])
            gap = last["centralized_baseline_accuracy"] - last["held_out_accuracy"]
            writer.writerow(
                [
                    dataset,
                    aggregator,
                    split,
                    alpha if alpha is not None else "",
                    attack,
                    last["round"],
                    f"{last['held_out_accuracy']:.4f}",
                    f"{last['centralized_baseline_accuracy']:.4f}",
                    f"{gap:.4f}",
                ]
            )
    print(f"wrote summary -> {summary_csv}")


if __name__ == "__main__":
    main()
