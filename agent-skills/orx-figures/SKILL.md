---
name: orx-figures
description: "Make charts and publication figures. Use before writing plotting code or fixing a figure."
---

Use interactive HTML for experiment metrics in chat and static vector figures
for a paper or export. Skip a chart when the data or templates do not fit, or
when a text summary communicates the result better.

## Non-negotiables

1. **Build at the final printed size.** Take `COLUMN`, `TEXT` or `WIDE` from
   the style module; never build big and rescale to fit.
2. **Vector out.** PDF for `\includegraphics`, SVG beside it for preview. A
   rasterized plot blurs under a reviewer's zoom; PNG is only for genuinely
   raster content: a photograph, a sample grid, a pixel-resolution attention map.
3. **Every number comes from a run.** Read metrics from the file `orx logs`
   locates (`orx-results`). Never plot a remembered, rounded or plausible number,
   or leave synthetic demo data in a script that ships.
4. **The caption is the title.** No `ax.set_title` on a paper figure; panel
   letters (**a**, **b**) name the parts of a multi-panel figure.
5. **Show the uncertainty, or say there is none.** Plot the interval across
   seeds and state the seed count in the caption; with one run, say "single seed".
6. **Label the axes with units**, and say whether "step" counts optimizer steps
   or tokens.
7. **Colorblind- and greyscale-safe.** Use the module's palette; never `jet`,
   `rainbow` or `hsv`, which invent structure that is not in the data.
8. **One sans-serif face in every figure, diagrams included.** `use_style()`
   defaults to a Helvetica-metric sans, what Nature and Science require and what
   survives 5-8pt tick labels; it need not match the body font. Pass
   `use_style(family="serif")` only for heavy math that should read as part of
   a serif-set page.

## Where the figure goes, and how to cite it

| The figure is for | Write it to | Cite it as |
| --- | --- | --- |
| A paper (`.tex`) | `figs/` beside the `.tex` in the working tree, since `\includegraphics` resolves paths relative to the source | `<file path="figs/loss_curve.pdf" />`, **no** `artifacts/` prefix |
| A report, a summary, an answer in chat | The topic or deliverable folder under the artifacts directory chosen per `orx-reports`, e.g. `transformer-sweep/figures/` | `<file path="artifacts/transformer-sweep/figures/loss_curve.pdf" />` |

**The tag must match the destination**: a worktree figure cited with an
`artifacts/` prefix is in neither root, and the user gets "File not found".
**Those are the only two destinations**: a chat file link resolves against
nothing else, so never write a figure you will cite into `/tmp` or a scratch
directory (logs and intermediate data may go there). Keep the generating script
beside its output (`figs/loss_curve.py` → `figs/loss_curve.pdf`,
`figs/loss_curve.svg`), so the figure can be corrected when a reviewer asks for
one more seed.

Show every new image figure inline in the handoff, SVGs and HTML charts
included, and keep its clickable file link: `![Description](artifacts/figure.svg)`.
Link CSV/JSON evidence alongside it, and reuse a run's ready-made chart rather
than regenerating it.

## Read exactly one figure-type reference

Pick by the question the figure answers, not by the shape you have in mind.

| The figure answers | Read |
| --- | --- |
| How does a metric move over training, and is the gap bigger than seed noise? | [references/curves.md](references/curves.md) |
| How does performance change with scale, and what does the trend predict? | [references/scaling.md](references/scaling.md) |
| Which method wins across benchmarks, or which ablated component mattered? | [references/comparison.md](references/comparison.md) |
| What is traded off against what: reward vs KL, quality vs cost or latency? | [references/pareto.md](references/pareto.md) |
| What does this 2D grid, confusion matrix, or sweep look like? | [references/matrix.md](references/matrix.md) |
| What is the method, architecture, or pipeline? | [references/diagram.md](references/diagram.md) |

Each template writes to `figs/`, the paper destination; for a report or chat
answer, adapt its script and output paths. Anything outside those six (a
photograph, a qualitative sample grid, a map) still obeys the non-negotiables
and the style module. Do not read references for figures you are not making.

## Read when

- An interactive chart for chat: [references/interactive.md](references/interactive.md).
- The first static figure in a project, before importing the style module: [references/setup.md](references/setup.md).
- Including a figure in a `.tex`: [references/paper-include.md](references/paper-include.md).
- Writing a paper figure's caption: [references/captions.md](references/captions.md).
- A figure with more than one panel: [references/multi-panel.md](references/multi-panel.md).
- Before handing over a static figure: [references/handover-checklist.md](references/handover-checklist.md).

If a reference beside this skill can't be read, `orx skill figures/<name>`
prints it.
