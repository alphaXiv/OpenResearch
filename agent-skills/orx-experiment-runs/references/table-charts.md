# Charts of table research

Benchmarks, sweeps and tasks by size report their results as table rows:

- **Sizes that span decades** (1k to 1M items, learning rates) get
  `x-scale="log"`, and times or slowdowns across them `y-scale="log"`:
  on linear axes everything below the largest size sits at zero.
- **One line per category**: when rows mix kinds (input distributions, tasks,
  datasets), `by="dist"` draws a series per kind; one line through all of them
  zigzags at each size and means nothing.
- **Two measures of one row** (the reference's time and the candidate's) are
  two `<series>` with labels, `ghost="false"` on the reference.

A sorting benchmark that prints one line per distribution and size, read into
the `timings` table by a pattern (`references/patterns.md`):

```html
<signal name="mine_ms" label="Merge sort" unit="ms" better="lower" />
<signal name="ref_ms" label="sorted()" unit="ms" better="lower" />
<view>
  <chart from="timings" x="n" y="mine_ms" by="dist" x-scale="log" y-scale="log" ghost="parent" />
  <table name="cases" columns="dist n slowdown correct" vs="final" />
</view>
```

```json
[{"regex": "^(?P<dist>\\w+)\\s+(?P<n>\\d+)\\s+(?P<ref_ms>[\\d.]+)\\s+(?P<mine_ms>[\\d.]+)", "row": "timings"}]
```
