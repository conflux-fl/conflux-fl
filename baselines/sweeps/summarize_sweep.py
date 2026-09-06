#!/usr/bin/env python3
"""Converts a sweep's JSONL output into a flat CSV (every round) and a
summary CSV (final-round accuracy per combination, aggregated across
seeds) — no dependencies beyond the standard library.

Produced by `cargo run -p conflux-baselines -- sweep ... --out FILE`.

The summary reports mean and standard deviation across seeds, not a
single number, because one seed measures a run rather than a method. On
this harness the run-to-run spread is wide enough that two aggregators
can trade places without either being better, so a difference smaller
than the spread is not a difference — the summary prints `n_seeds` next
to every row to make that judgeable rather than assumed.

Usage:
    python3 summarize_sweep.py results.jsonl
"""

import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

# One row per combination; the seed is what the row aggregates over.
KEY_FIELDS = ("dataset", "aggregator", "split", "dirichlet_alpha", "attack")


def fmt(value: float | None, places: int = 4) -> str:
    """A number, or an empty cell — a spreadsheet reads `` as missing and
    `0.0000` as measured, and those are different claims."""
    return "" if value is None else f"{value:.{places}f}"


def mean_std(values: list[float]) -> tuple[float | None, float | None]:
    """Mean and population standard deviation, or `(None, None)` when
    there is nothing to average.

    `pstdev` rather than `stdev`: these seeds are the whole set of runs
    performed, not a sample drawn from a larger population, and `stdev`
    additionally raises on a single value where the honest answer is
    "no spread observed yet".
    """
    if not values:
        return None, None
    if len(values) == 1:
        return values[0], None
    return statistics.fmean(values), statistics.pstdev(values)


def final_round_per_seed(group: list[dict]) -> list[dict]:
    """The last round each seed reached.

    Seeds can reach different final rounds — the evaluator polls, so it
    records some rounds and not others — and taking one global maximum
    would silently drop every seed that stopped earlier.
    """
    by_seed: dict[int, dict] = {}
    for row in group:
        seed = row.get("seed", 0)
        if seed not in by_seed or row["round"] > by_seed[seed]["round"]:
            by_seed[seed] = row
    return list(by_seed.values())


def main() -> None:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <results.jsonl>", file=sys.stderr)
        sys.exit(1)

    in_path = Path(sys.argv[1])
    rows = [json.loads(line) for line in in_path.read_text().splitlines() if line.strip()]
    if not rows:
        print(f"no rows in {in_path}", file=sys.stderr)
        sys.exit(1)

    # Files written by earlier versions lack the later fields, so the
    # header is the union rather than the first row's keys.
    fieldnames: list[str] = []
    for row in rows:
        for key in row:
            if key not in fieldnames:
                fieldnames.append(key)

    full_csv = in_path.with_suffix(".csv")
    with full_csv.open("w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames, restval="")
        writer.writeheader()
        writer.writerows(rows)
    print(f"wrote {len(rows)} rows -> {full_csv}")

    groups: dict[tuple, list[dict]] = defaultdict(list)
    for r in rows:
        # Every key field defaults, so results written before the sweep
        # grew a dimension still summarize.
        groups[tuple(r.get(k, "none" if k == "attack" else None) for k in KEY_FIELDS)].append(r)

    summary_csv = in_path.with_name(in_path.stem + ".summary.csv")
    with summary_csv.open("w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(
            [
                *KEY_FIELDS,
                "n_seeds",
                "final_round",
                "held_out_accuracy_mean",
                "held_out_accuracy_std",
                "client_acc_min_mean",
                "client_acc_std_mean",
                "centralized_baseline_accuracy",
                "accuracy_gap_vs_baseline",
            ]
        )
        for key, group in sorted(groups.items(), key=lambda kv: tuple(str(p) for p in kv[0])):
            dataset, aggregator, split, alpha, attack = key
            finals = final_round_per_seed(group)

            acc_mean, acc_std = mean_std([r["held_out_accuracy"] for r in finals])
            worst_mean, _ = mean_std(
                [r["client_acc_min"] for r in finals if r.get("client_acc_min") is not None]
            )
            spread_mean, _ = mean_std(
                [r["client_acc_std"] for r in finals if r.get("client_acc_std") is not None]
            )
            baseline, _ = mean_std(
                [
                    r["centralized_baseline_accuracy"]
                    for r in finals
                    if r.get("centralized_baseline_accuracy") is not None
                ]
            )
            gap = None if baseline is None or acc_mean is None else baseline - acc_mean

            writer.writerow(
                [
                    dataset,
                    aggregator,
                    split,
                    alpha if alpha is not None else "",
                    attack,
                    len(finals),
                    max(r["round"] for r in finals),
                    fmt(acc_mean),
                    fmt(acc_std),
                    fmt(worst_mean),
                    fmt(spread_mean),
                    fmt(baseline),
                    fmt(gap),
                ]
            )
    print(f"wrote summary -> {summary_csv}")

    seeds = {r.get("seed", 0) for r in rows}
    if len(seeds) == 1:
        print(
            "\nnote: one seed. `held_out_accuracy_std` is empty because a\n"
            "single run has no spread to report — pass several `--seeds` to\n"
            "the sweep before treating a difference between methods as real.",
            file=sys.stderr,
        )
    else:
        print(
            f"\n{len(seeds)} seeds. Differences smaller than the reported std are\n"
            "not differences.",
            file=sys.stderr,
        )


if __name__ == "__main__":
    main()
