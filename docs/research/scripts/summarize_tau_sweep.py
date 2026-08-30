#!/usr/bin/env python3
"""Summarizes a clip-radius sweep — the one thing summarize.py can't do.

summarize.py groups by (aggregator, attack), which is the right key for
every experiment where the aggregator name fully determines the
configuration. It isn't for Experiment 2.7 part 2, where every row says
`centered_clipping` and the thing that varies is `clip_radius`. Grouping
those rows by aggregator would average four different taus into one
number that describes none of them.

Rather than adding a conditional grouping key to the shared summarizer
(and changing the column layout of every already-committed summary this
directory has produced), this is a separate script for a separate
question. Same stdlib-only constraint, same normal-approximation CI.

Usage:
    python3 summarize_tau_sweep.py path/to/results_tau_sweep.jsonl
"""

import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path


def mean_ci(values: list[float]) -> tuple[str, str]:
    """Mean and 95% CI half-width, matching summarize.py's convention:
    1.96*stdev/sqrt(n), blank when n < 2 (a CI over one sample is
    undefined, and printing 0 would read as 'no variance')."""
    if not values:
        return "", ""
    mean = statistics.fmean(values)
    if len(values) < 2:
        return f"{mean:.6f}", ""
    ci = 1.96 * statistics.stdev(values) / (len(values) ** 0.5)
    return f"{mean:.6f}", f"{ci:.6f}"


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <results_tau_sweep.jsonl>", file=sys.stderr)
        sys.exit(1)

    in_path = Path(sys.argv[1])
    rows = [json.loads(line) for line in in_path.read_text().splitlines() if line.strip()]
    if not rows:
        print(f"{in_path} is empty", file=sys.stderr)
        sys.exit(1)

    groups: dict[tuple[str, str, float], list[dict]] = defaultdict(list)
    for r in rows:
        groups[(r["aggregator"], r["attack"], r["clip_radius"])].append(r)

    out_path = in_path.with_suffix(".summary.csv")
    with out_path.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(
            [
                "aggregator",
                "attack",
                "clip_radius",
                "n_rows",
                "n_seeds",
                "mean_distance",
                "distance_ci95",
                "mean_asr",
                "asr_ci95",
                "first_round_distance",
                "final_round_distance",
            ]
        )
        for (aggregator, attack, tau), group in sorted(groups.items()):
            distances = [g["distance_from_true_value"] for g in group]
            asrs = [g["asr"] for g in group if g["asr"] is not None]
            by_round = sorted(group, key=lambda g: g["round"])
            mean_distance, distance_ci = mean_ci(distances)
            mean_asr, asr_ci = mean_ci(asrs)
            writer.writerow(
                [
                    aggregator,
                    attack,
                    tau,
                    len(group),
                    len({g["seed"] for g in group}),
                    mean_distance,
                    distance_ci,
                    mean_asr,
                    asr_ci,
                    f"{by_round[0]['distance_from_true_value']:.6f}",
                    f"{by_round[-1]['distance_from_true_value']:.6f}",
                ]
            )

    print(f"wrote summary -> {out_path}")


if __name__ == "__main__":
    main()
