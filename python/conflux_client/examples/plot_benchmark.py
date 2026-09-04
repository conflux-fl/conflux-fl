#!/usr/bin/env python3
"""Plots benchmark.py's results — two figures from one JSONL file:

  1. Accuracy vs. round, one line per aggregator, one panel per split —
     shows convergence speed, not just a final number.
  2. Final accuracy vs. aggregator, grouped bars by split — the
     "at-a-glance comparison" figure most useful for a README/report.

Requires matplotlib (not a dependency of the framework itself — only
needed to generate these plots):
    pip install matplotlib

Usage:
    python3 plot_benchmark.py results.jsonl [output_prefix]
"""

import json
import sys
from pathlib import Path

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt


def main() -> None:
    if len(sys.argv) < 2:
        print(f"usage: {sys.argv[0]} <results.jsonl> [output_prefix]", file=sys.stderr)
        sys.exit(1)

    in_path = Path(sys.argv[1])
    prefix = sys.argv[2] if len(sys.argv) > 2 else str(in_path.with_suffix(""))
    rows = [json.loads(line) for line in in_path.read_text().splitlines() if line.strip()]

    splits = sorted({(r["split"], r["dirichlet_alpha"]) for r in rows}, key=lambda s: (s[0], s[1] or 0))
    aggregators = sorted({r["aggregator"] for r in rows})
    colors = plt.rcParams["axes.prop_cycle"].by_key()["color"]
    agg_color = {a: colors[i % len(colors)] for i, a in enumerate(aggregators)}

    # --- Figure 1: accuracy vs. round, one panel per split ---
    fig, axes = plt.subplots(1, len(splits), figsize=(5.5 * len(splits), 4.5), squeeze=False)
    axes = axes[0]
    for ax, (split, alpha) in zip(axes, splits):
        label = "IID" if split == "iid" else f"Non-IID (Dirichlet α={alpha})"
        for aggregator in aggregators:
            series = sorted(
                (r["round"], r["held_out_accuracy"])
                for r in rows
                if r["aggregator"] == aggregator and r["split"] == split and r["dirichlet_alpha"] == alpha
            )
            if not series:
                continue
            xs, ys = zip(*series)
            ax.plot(xs, ys, marker="o", markersize=3, label=aggregator, color=agg_color[aggregator])
        baseline_rows = [r for r in rows if r["split"] == split and r["dirichlet_alpha"] == alpha]
        if baseline_rows:
            ax.axhline(
                baseline_rows[0]["centralized_baseline_accuracy"],
                color="gray",
                linestyle="--",
                linewidth=1,
                label="centralized baseline",
            )
        ax.set_title(label)
        ax.set_xlabel("round")
        ax.set_ylabel("held-out accuracy")
        ax.set_ylim(0, 1)
        ax.grid(alpha=0.3)
    axes[-1].legend(fontsize=8, loc="lower right")
    fig.tight_layout()
    fig.savefig(f"{prefix}_convergence.png", dpi=150)
    print(f"wrote {prefix}_convergence.png")

    # --- Figure 2: final accuracy, grouped bars ---
    final_acc: dict[tuple, float] = {}
    for (split, alpha) in splits:
        for aggregator in aggregators:
            group = [
                r
                for r in rows
                if r["aggregator"] == aggregator and r["split"] == split and r["dirichlet_alpha"] == alpha
            ]
            if group:
                final_acc[(split, alpha, aggregator)] = max(group, key=lambda r: r["round"])["held_out_accuracy"]

    fig2, ax2 = plt.subplots(figsize=(1.6 * len(aggregators) * len(splits) + 2, 5))
    width = 0.8 / len(splits)
    x = range(len(aggregators))
    for i, (split, alpha) in enumerate(splits):
        label = "IID" if split == "iid" else f"Non-IID (α={alpha})"
        heights = [final_acc.get((split, alpha, a), 0) for a in aggregators]
        offsets = [xi + (i - (len(splits) - 1) / 2) * width for xi in x]
        ax2.bar(offsets, heights, width=width, label=label)
    ax2.set_xticks(list(x))
    ax2.set_xticklabels(aggregators, rotation=20, ha="right")
    ax2.set_ylabel("final held-out accuracy")
    ax2.set_ylim(0, 1)
    ax2.legend(fontsize=9)
    ax2.grid(axis="y", alpha=0.3)
    fig2.tight_layout()
    fig2.savefig(f"{prefix}_final_accuracy.png", dpi=150)
    print(f"wrote {prefix}_final_accuracy.png")


if __name__ == "__main__":
    main()
