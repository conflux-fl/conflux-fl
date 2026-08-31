# DSS paper

`dss.tex` — a standalone LaTeX write-up of the DSS research line, meant
to be readable in one sitting by someone who will not open the lab
notebook.

It is **not** a replacement for
[`../temporal-consistency-aggregation.md`](../temporal-consistency-aggregation.md).
The notebook's chronology is what makes its self-corrections legible —
which conclusion was believed when, and what overturned it. The paper
states the claims; the notebook shows the work. Keep both.

## Building

```bash
pdflatex dss.tex && pdflatex dss.tex   # twice, for cross-references
```

No `.bib` file and no `bibtex` pass — the bibliography is an inline
`thebibliography` environment. Figures resolve via `\graphicspath` to
`../figures/`, so build from this directory.

Packages used are all in a standard TeX Live install: `geometry`,
`amsmath`, `amssymb`, `booktabs`, `graphicx`, `caption`, `multirow`,
`array`, `xcolor`, `url`, `hyperref`.

**Never compiled.** There is no TeX toolchain on the machine this was
written on. Structure was validated mechanically (environment nesting,
brace and math-mode balance, no unescaped specials outside `verbatim`,
every `\includegraphics` target present on disk) but a real compile has
not run. Expect to fix trivia on the first build.

## Where the numbers come from

Every figure in the paper is copied from a `../results/*.summary.csv`
produced by a committed script in `../scripts/`, consolidated in
[`../BASELINES.md`](../BASELINES.md). Appendix A maps each section to its
script and results file. Nothing is typed from memory.

## What the paper deliberately includes

A section (§10, "Conclusions Overturned by Measurement") for the four
claims this research line stated and then disconfirmed — two of them its
own diagnoses. It is there because the discipline that produced them is
the same one that produced the positive results, and a paper that
reported only the latter would be describing a different project.

Open questions, including the two blocked on a research judgment rather
than on code, are tracked in [`../tasks.json`](../tasks.json), not here.
