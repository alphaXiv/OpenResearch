# Multi-panel figures

Roughly a quarter of figures in current ML papers are multi-panel: `(a)`/`(b)`
subpanels, or a `Left:`/`Right:` pair. Panels are a composition choice that
cuts across every figure type, and they go wrong in consistent ways.

- **Label every panel** `(a)`, `(b)`, … at the top-left (`panel_labels()`).
  Prose refers to "Fig. 2b"; without letters the only way to name a panel is by
  position, which breaks the moment one moves.
- **Share the axis when panels share a quantity** (`sharey=True`), and let the
  shared axis carry one label on the panel that shows the ticks. Two panels of
  the same metric on silently different ranges is the multi-panel version of a
  truncated bar axis: the reader compares heights that are not comparable.
- **One legend for the figure**, not one per panel. Put it under the panels or
  outside the last one.
- **Panels read in argument order**: left to right, top to bottom. Panel (a)
  should be the one the text discusses first.
- **If the panels do not support one claim, they are separate figures.** A
  2×3 grid of unrelated plots is a contact sheet, and its caption will read as
  a list rather than a sentence.

Build them with `figure_grid(nrows, ncols, width=TEXT, sharey=True)`, which
sizes the whole grid to the final printed width.
