#!/usr/bin/env python3
"""Converts an experiment JSONL results file into a CSV summary — no
dependencies beyond the standard library, so it runs anywhere without
needing the E2E harnesses' own venv/torch stack. Two outputs:
  1. <input>.csv — every row, flattened, ready for pandas/Excel/whatever.
  2. <input>.summary.csv — mean +/- 95% CI (normal approximation,
     1.96*stdev/sqrt(n) -- a stdlib-only simplification of the more
     correct t-distribution CI, adequate once n>=5 seeds/repeats) of
     distance_from_true_value and ASR per (aggregator, attack) -- the
     table docs/research/temporal-consistency-aggregation.md's Section 3
     needs. CI columns are blank when n<2 (undefined) or every row shares
     one seed (single-run number, not yet a statistically rigorous one --
     see that document's Section 7.1 item 4).

Usage:
    python3 summarize.py path/to/results.jsonl
"""

import csv
import json
import statistics
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

    fieldnames = list(rows[0].keys())
    full_csv = in_path.with_suffix(".csv")
    with full_csv.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)
    print(f"wrote {len(rows)} rows -> {full_csv}")

    groups: dict[tuple[str, str], list[dict]] = defaultdict(list)
    for r in rows:
        groups[(r["aggregator"], r["attack"])].append(r)

    def ci95(values: list[float]) -> str:
        n = len(values)
        if n < 5:
            return ""  # too few independent repeats for a meaningful CI
        return f"{1.96 * statistics.stdev(values) / (n ** 0.5):.6f}"

    summary_csv = in_path.with_name(in_path.stem + ".summary.csv")
    with summary_csv.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(
            [
                "aggregator",
                "attack",
                "n_rows",
                "n_seeds",
                "mean_distance_from_true_value",
                "ci95_distance_from_true_value",
                "mean_asr",
                "ci95_asr",
                "first_round_distance",
                "last_round_distance",
            ]
        )
        for (aggregator, attack), group in sorted(groups.items()):
            distances = [g["distance_from_true_value"] for g in group]
            asrs = [g["asr"] for g in group if g["asr"] is not None]
            n_seeds = len(set(g["seed"] for g in group))
            group_sorted = sorted(group, key=lambda g: g["round"])
            writer.writerow(
                [
                    aggregator,
                    attack,
                    len(group),
                    n_seeds,
                    f"{statistics.mean(distances):.6f}",
                    ci95(distances),
                    f"{statistics.mean(asrs):.6f}" if asrs else "",
                    ci95(asrs) if asrs else "",
                    f"{group_sorted[0]['distance_from_true_value']:.6f}",
                    f"{group_sorted[-1]['distance_from_true_value']:.6f}",
                ]
            )
    print(f"wrote summary -> {summary_csv}")


if __name__ == "__main__":
    main()
