# Interactive charts in chat

When a visualization helps explain experiment metrics in chat, use the bundled
offline HTML chart. Start with one useful view. Add a dropdown only when the
same data benefits from multiple distinct views; never add views just to fill
the selector. The template handles chart styling, controls, and interactions.
Use real run data, preserve raw values, and state the seed count.

Vendor both files beside the rendering script:

```sh
orx skill figures/assets/orx-chart.html > artifacts/orx-chart.html
orx skill figures/assets/orx_chart.py > artifacts/orx_chart.py
```

```python
from orx_chart import render_chart
render_chart("artifacts/loss.html", title="Learning-rate comparison",
    subtitle="20 steps · single seed",
    metrics=[dict(key="loss", label="Validation loss", unit="nats/token")],
    series=[dict(name="LR 0.001", points=baseline_rows),
            dict(name="LR 0.002", points=comparison_rows)])
```

Each point is a dict with numeric `step` and metric values. A missing or null
metric is skipped. Pass at least one finite point per metric; use matching steps
for run comparisons. Import from the directory where you vendored the helper.
Keep HTML self-contained: no CDN scripts, external fonts, or network dependency.

Embed the result inline using image syntax (HTML opens as a sandboxed figure):

```markdown
![Learning-rate comparison](artifacts/loss.html)
```

## Graph templates

One graph is visible at a time. The shared dropdown chooses the view; do not
build composite dashboards or append tables to this component.

Use `orx_chart.render_chart` with `title`, `subtitle`, `metrics`, and `series`.
Each view has a unique `key`, `label`, `type`, `unit`, optional `xLabel` and
`yLabel`, and optionally its own `series`. A series has `name`, `points`,
optional `color` and `dash`. Points use numeric `step` for x and the view's
`key` for y. Use real measurements and state seed count in the subtitle.

| ORX family | Interactive template and data |
| --- | --- |
| Learning curves | `line`; measured `lower`/`upper` bounds add a confidence band. `area` fills to zero for signed differences. |
| Scaling laws | `scaling`; positive x/y coordinates, logarithmic axes by default. Supply separately computed fitted samples as `series.fit`; `fitDash` distinguishes extrapolation. State equation, fit range and uncertainty in the subtitle. The renderer does not estimate a law. |
| Comparisons / ablations | `bar` groups methods at shared numeric category positions with point `label` for ticks, zero baseline. `dot` shows horizontal deltas with optional measured `lower`/`upper` whiskers and category `label`. |
| Pareto trade-offs | `pareto`; numeric x/y points, defaults minimize x/maximize y; set `minimizeX`/`maximizeY` for other directions. All measured points remain visible; only non-dominated points are connected per series. Name directions and state the frontier is a guide. |
| Matrices | `heatmap` with one series per row; `columns` supplies column labels. Sequential viridis colors by default; `colorScale="diverging"` centers signed values on zero. Missing cells remain gray. `confusion` uses square cells; supply normalized values, `colorMin=0`, `colorMax=1`, and state normalization/support. |
| Method diagrams | `diagram` uses `nodes` with `id`, `label`, integer `column`/`row`, optional `accent`/`description`; `edges` have `from`, `to`, optional `label`/`dashed`. Clicking a node highlights adjacent connections; Escape restores the diagram. |

`scatter` is also available for measured relationships. Curves, scatter,
scaling and Pareto plots may set `xScale` or `yScale` to `log`. Bars and areas
require a linear y-axis because their baseline is zero. Numeric curves, scatter
and Pareto plots use mouse drag selection zoom and show Reset only after zoom.
Matrix cells, diagram nodes and dot intervals expose exact data on hover/focus.
Download exports the current graph as SVG. Do not invent uncertainty or scaling
data just to demonstrate a template; a single-seed demo can show single-seed
curves, comparisons, measured trade-offs, a matrix, and its actual method
diagram.
