# Writing a run's view

## Choose the measures

- **The headline is the number in the first sentence of the result.** Train on
  loss and judge by accuracy, and the headline is accuracy. If it only exists
  late, list a stand-in after it: `headline="accuracy loss"`.
- **Declare `better`** on every measure with a direction: without it there is
  no comparison with the parent.
- **One name, one series, the whole run.** A later stage measuring the same
  thing names it apart: `sft_loss`, not `loss` again. Never rename a measure
  once children exist; change its `label`.
- **Results that arrive one at a time are rows** (tasks, categories, operating
  points, sweep points): a table's first column is its key, its second the
  value compared with the parent.

## Elements

```sh
orx exp view set <expId> --file view.xml
```

| Element | Attributes |
| --- | --- |
| `signal` | `name`, `label`, `unit`, `digits` (0–12), `better="lower\|higher"`, `smooth` (above 0, at most 1; noisy curves only), `as="duration\|compact"`, `scale` (a factor, `100` for percent), `sign="true"`, `pick="latest\|best"` (`best` needs `better`), `vs`, `labels='{"1":"valid","0":"invalid"}'` |
| `view` | `headline="a b"`: the first with a number is shown; a signal that went wrong (not finite, or zero where the parent's is not) leads instead, wherever it is listed |
| `chart` | `y="a b"`, `ghost="parent\|false"` (default `parent`), `x-range="run\|parent"`, `x-scale="log"`, `y-scale="log"`, `mark="line\|points\|bars"`; with `from="table"`, `x`/`y` are its columns, `by="column"` draws a series per value, and `<series x= y= label= ghost=/>` children give each series its own columns |
| `stats` | `of="a b"`, `vs` |
| `table` | `name`, `columns="a b"`, `last="5"`, `vs="parent\|step\|final"` |
| `text` | Plain text with optional `{signal}` tokens and `**bold**`, up to 2,048 bytes |
| `cells` | `from="table"`, newest 256 retained outcome rows in completion order |
| `bars` | `from="table"`, `by="column"`, correct share by category over retained outcome rows |
| `media` | `glob="samples/*.png"`, `last="6"` (1–6, default 6) |

A chart over signals uses reported steps when available and elapsed time
otherwise. Users can change `x-scale`, `y-scale` and `smooth` from a chart's
menu, so read the current view with `orx exp view get` and keep those choices
when you run `orx exp view set`.

Every block also takes `phase` and `title`. The strip only picks a phased chart
during its stage. The run page and embedded figures can show blocks from any
stage.

Declare a `signal` for every measure the view names and every column the user
should read by a label. One chart answers one question: measures on different
scales get a chart each. A chart draws at most 8 series and a stats block
takes up to 32 names; show about three stats, the headline first. The strip
shows the headline and the first active chart with two points. A result
citation draws the view's first block (`orx-figures`). Use the result message
for the interpretation of a particular run.

```xml
<signal name="val_loss" digits="2" />
<view headline="val_loss">
  <text>Loss is **{val_loss}**. Lower is better.</text>
  <cells from="items" />
  <bars from="items" by="category" />
</view>
```

Text binds the latest value of each named signal. Before it reports, the token
shows a dash. Escape XML characters as in `Loss &lt; 1`.

## Comparing with the parent

How a run meets its parent is yours to choose, per experiment:

- **`vs`** on `stats`, `table`, or a `signal` as its default: `step` compares
  with the parent at the same stage and step, `final` with the value the parent
  ended with. A signal's `final` comparison is available once both runs end.
  Left out, it is `step` while the run goes and `final` once it ends. Use
  `final` when the parent's result is the bar to clear, `step` when the
  question is the path (does it learn faster?). A table compares only with
  `vs`; `vs="parent"` takes the default.
- **`ghost="parent"`** (the default) draws the parent's same series dashed
  under each line. Put `ghost="false"` on a chart to hide its parent curves, or
  on a `<series>` the parent would only repeat, such as a reference measured
  the same way in every run.
- **`x-range`** on a chart over steps: `run` (the default) spans this run's
  total and cuts the parent off at the edge; `parent` spans the parent's run
  too, for a child that stops early or runs longer.

## Read when

- `references/table-charts.md` — when results are table rows across sizes, kinds of input, or a reference and a candidate
- `references/outcomes.md` — to show each item's correct or wrong outcome as `cells`, or the correct share by category as `bars`
- `references/media.md` — to preview images a local run generates
- `references/examples/pretraining.md` — pretraining with an eval and SFT, plan known up front
- `references/examples/eval-by-category.md` — an eval reporting accuracy per category
- `references/examples/frontier.md` — a speed and size frontier against a reference built in the run
- `references/examples/sweep.md` — a learning-rate sweep and refit inside one run
- `references/examples/frozen-solver.md` — a frozen binary read by patterns
