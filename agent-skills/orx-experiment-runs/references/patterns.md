# Reading a log you can't edit

Patterns are for code no experiment may change: a harness that measures every
experiment the same way, a vendored tool. The baseline's own script is yours:
report from it with `::orx::` lines, which survive a reworded log and can count
any stage. For frozen code, a JSON list, one object per kind of line, set
beside the view, reads its log:

```sh
orx exp view set <expId> --pattern patterns.json
```

Named captures `step`, `of`, `t` and `spend` mean what they do on an `::orx::`
line; `name` with `value` makes a measure named by the log; any other capture
is a measure, or a cell when the pattern has `row`. `phase`, `unit` and `row`
are captured or fixed per pattern. Rust regex syntax without look-around or
backreferences; the first pattern that matches a line reads it. A stage whose
lines carry no `step` shows no progress.

A pattern's rows go to the table its `row` names. Rows the code reports under
the same name join that table, so a result both printed and reported counts
twice: give a pattern's table a name the code does not use.

- `references/examples/frozen-solver.md` — a frozen solver's stages and measures read entirely by patterns
- `references/table-charts.md` — a pattern that reads benchmark lines into table rows
