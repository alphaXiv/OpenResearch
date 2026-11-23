# Outcome cells and category bars

`<cells from="items" />` and `<bars from="items" by="category" />` draw from a
table of per-item outcomes. Emit one row per finished item. Duplicate rows each count.

```text
::orx:: {"row":"items","m":{"category":"algebra","outcome":"correct"}}
```

Use `correct`, `wrong`, `error` or `unknown` as strings. A fixed `row:"items"`
pattern can capture `category` and `(?P<outcome>correct|wrong|error|unknown)`.
Cells count missing and unrecognized outcomes as unknown. Bars show the most
recent 32 distinct categories at most, counting all their retained rows.

The rate is correct divided by correct, wrong and error combined. Unknown and
unrecognized outcomes are excluded. A category with no classified rows shows a
dash for its rate. Report any overall or cumulative accuracy as a separate
signal for the watcher or for a chart.
