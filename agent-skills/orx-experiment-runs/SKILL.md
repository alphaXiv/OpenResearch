---
name: orx-experiment-runs
description: "Make a run followable from launch to result: declare its stages, report progress and the measures that answer its question, write its view and how it compares with the parent, read a log you can't edit with patterns, check the first minutes, change any of it mid-run. Use before a baseline's first launch, when a run's progress or chart misleads, and when the user asks to change what a run shows."
---

The user follows a run without asking you: a line and a strip above the chat
box, a tile on the tree, the run's page. Each draws the run's stages as one bar,
its headline against the parent, and the view's first blocks. All of it comes
from what you add to the baseline's code and set on it before its first launch:

| What | Lives in | You set it with |
| --- | --- | --- |
| the stages, in order, with their totals | the experiment, or the run | `--config plan.json`, or `orx_run.plan` |
| progress and measures | the code | `::orx::` lines, or `orx_run` |
| the view and its comparisons | the experiment | `--file view.xml` |
| patterns for a log you can't edit | the experiment | `--pattern patterns.json` |

```sh
orx exp view set <expId> --config plan.json --file view.xml   # any of --config, --file, --pattern
orx exp view get <expId>
```

A child starts with a copy of its parent's view, patterns and plan, and
inherits its code. A later `view set` changes only the experiment it names.

## 1. Read the entry point

Read the script the run command starts, end to end, with what it calls. List
its stages in order, setup included: install, download, preprocess, train,
eval, export. For each, its amount of work (steps, files, tasks) and whether
that number is known now or only once the run computes it. A stage is a wait
the user sits through; an eval every 500 steps inside training is a measure of
`train`, not a stage of its own.

## 2. Declare the plan

Declare every stage so the bar spans the whole run from its first second,
instead of filling up before eval starts. Totals you know go in the config:

```json
{"plan": [{"phase": "install"},
          {"phase": "download", "of": 9, "unit": "shards"},
          {"phase": "train", "of": 5000, "unit": "steps"},
          {"phase": "eval", "of": 22, "unit": "tasks"}]}
```

When only the run knows a total (iterations from a FLOP budget, files found on
disk), it declares the plan itself once it knows, before the first stage
starts; a run's own plan replaces the experiment's for that run:

```python
orx.plan(["install", {"phase": "train", "of": iterations, "unit": "steps"}, "eval"])
```

A stage that recurs is listed again where it recurs, never twice in a row.
The bar times each stage by the parent's run of it. A stage the run reaches
outside the plan shows as unplanned, and a planned stage it ends without shows
as skipped: both mean the plan is wrong, so fix it.

## 3. Report progress and measures

Add the reports to the baseline's code and commit them before its first
launch. A run reports with `::orx::` lines on its output:

```text
::orx:: {"phase":"download","step":3,"of":9,"unit":"shards"}
::orx:: {"step":1240,"of":5000,"t":1824.5,"m":{"loss":2.31,"val_bpb":1.17}}
::orx:: {"row":"tasks","m":{"task":"arc_easy","accuracy":0.62}}
```

`phase` starts a stage, and omitted fields keep their last value. `step` is
completed work out of `of`; `t` is seconds on any monotonic clock (only
differences count); measures go in `m`; `row` adds a row to that table;
`spend` is cumulative paid-service USD; `plan` is the stage list of section 2.

Python vendors the stdlib-only helper and commits it with the code:

```sh
orx skill compute/assets/orx_run.py > orx_run.py
```

```python
import orx_run as orx
run = orx.progress(total=5000, unit="steps")   # right after its stage starts
run.step(step, loss=loss, val_loss=val)         # every step; one report a second goes out
with run.phase("eval", total=22, unit="tasks") as p:
    for done, task in enumerate(tasks, 1):
        p.row("tasks", task=task, accuracy=score(task))   # one row per finished unit
        p.step(done)                                      # a row alone moves no bar
run.spend(cumulative_usd)
```

It reports from global rank zero, flushes a non-finite measure at once, and
never marks a run finished. Resume with `progress(total=5000, start=3000)`.
A shell script names the stages and relays the progress of tools it can't
change; the Python it starts then only calls `progress(total=…)`:

```sh
report() { printf '::orx:: %s\n' "$1"; }
report '{"phase":"download","step":0,"of":9,"unit":"shards"}'
python download.py | while IFS= read -r line; do
    printf '%s\n' "$line"
    case $line in "Saved shard"*) n=$((n + 1)); report "{\"step\":$n}" ;; esac
done
report '{"phase":"train"}'
PYTHONUNBUFFERED=1 python train.py
```

Without `PYTHONUNBUFFERED=1`, piped output arrives late and in bursts.

## 4. Choose the measures

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

## 5. Write the view

```sh
orx exp view set <expId> --file view.xml
```

| Element | Attributes |
| --- | --- |
| `signal` | `name`, `label`, `unit`, `digits` (0–12), `better="lower\|higher"`, `smooth` (above 0, at most 1; noisy curves only), `as="duration\|compact"`, `scale` (a factor, `100` for percent), `sign="true"`, `pick="latest\|best"` (`best` needs `better`), `vs`, `labels='{"1":"valid","0":"invalid"}'` |
| `view` | `headline="a b"`: the first with a number is shown |
| `chart` | `y="a b"`, `x` (`step`, the default, or `elapsed`), `ghost="parent"`, `x-range="run\|parent"`, `x-scale="log"`, `y-scale="log"`, `mark="line\|points\|bars"`; with `from="table"`, `x`/`y` are its columns, `by="column"` draws a series per value, and `<series x= y= label= ghost=/>` children give each series its own columns |
| `stats` | `of="a b"`, `vs` |
| `table` | `name`, `columns="a b"`, `last="5"`, `vs="parent\|step\|final"` |

Every block also takes `phase` (shown only during that stage) and `title`.
Declare a `signal` for every measure the view names and every column the user
should read by a label. One chart answers one question: measures on different
scales get a chart each. At most 8 series a chart and three stats, the
headline first. The strip shows the headline and the current stage's first
block: put first the picture that would make you stop the run. No colours,
sizes or HTML: what a block can't say goes in the result message
(`orx-figures`).

How a run meets its parent is yours to choose, per experiment:

- **`vs`** on `stats`, `table`, or a `signal` as its default: `step` compares
  with the parent at the same stage and step, `final` with the value the parent
  ended with. Left out, it is `step` while the run goes and `final` once it
  ends. Use `final` when the parent's result is the bar to clear, `step` when
  the question is the path (does it learn faster?). A table compares only with
  `vs`; `vs="parent"` takes the default.
- **`ghost="parent"`** draws the parent's same series dashed under each line.
  Put `ghost="false"` on a `<series>` the parent would only repeat, such as a
  reference measured the same way in every run.
- **`x-range`** on a chart over steps: `run` (the default) spans this run's
  total and cuts the parent off at the edge; `parent` spans the parent's run
  too, for a child that stops early or runs longer.

## 6. A log you can't edit

Patterns are for code no experiment may change: a harness that measures every
experiment the same way, a vendored tool. The baseline's own script is yours:
report from it with `::orx::` lines, which survive a reworded log and can count
any stage. For frozen code, a JSON list, one object per kind of line, set
beside the view, reads its log (the frozen solver below).

Named captures `step`, `of`, `t` and `spend` mean what they do on an `::orx::`
line; `name` with `value` makes a measure named by the log; any other capture
is a measure, or a cell when the pattern has `row`. `phase`, `unit` and `row`
are captured or fixed per pattern. Rust regex syntax without look-around or
backreferences; the first pattern that matches a line reads it. A stage whose
lines carry no `step` shows no progress.

## 7. Check the first minutes

After the baseline's first launch, wait until its log shows the first stage
and a measure (`orx exp status <expId>` every half minute), then:

```sh
orx exp view check <runId>
```

It folds the run's real log: the stages it read, the measures and tables
reported, what each pattern matched with its first line, and what the view
still waits for. Repair the view, plan or patterns with `view set`: they apply
to the run in flight. A report missing from the code means cancelling, fixing
and relaunching, which is cheap now and costly an hour in. Once the run reads
right, end your turn: results arrive as messages, and a user who asks mid-run
to change what it shows reaches you only between turns. Never block on
`orx exp wait`.

## 8. Change it while it runs

The user may ask mid-run to chart something else or to compare differently,
and the run itself may show that the view misleads: a stage outside the plan,
a stat compared with the wrong point of the parent, a curve flattened by one
early spike (`y-scale="log"`). Change it then, on each experiment it concerns
(siblings each hold their own copy):

- `--file`: every open page redraws from the new view.
- `--pattern`: the run's kept log is read again with them.
- `--config`: the bar redraws, for runs that declared no plan of their own.

What the code prints is fixed for that run: a measure it never reports needs a
code change, and the next run.

## Worked examples

### Pretraining with an eval and SFT

The plan is known up front; `train`, `eval` and `sft` each report.

```json
{"plan": [{"phase": "download", "of": 9, "unit": "shards"},
          {"phase": "train", "of": 5000, "unit": "steps"},
          {"phase": "eval", "of": 22, "unit": "tasks"},
          {"phase": "sft", "of": 600, "unit": "steps"}]}
```

```html
<signal name="val_bpb" label="Val bpb" digits="3" better="lower" />
<signal name="loss" label="Train loss" digits="3" smooth="0.3" better="lower" />
<signal name="tok_per_sec" label="Tokens/s" as="compact" better="higher" />
<signal name="task" label="Task" />
<signal name="accuracy" label="Accuracy" digits="3" better="higher" />
<signal name="core" label="CORE" digits="3" better="higher" />
<signal name="sft_loss" label="SFT loss" digits="3" smooth="0.3" better="lower" />
<view headline="core val_bpb">
  <chart y="val_bpb" ghost="parent" phase="train" />
  <chart y="loss" phase="train" />
  <stats of="val_bpb loss tok_per_sec" phase="train" />
  <table name="tasks" columns="task accuracy" vs="final" phase="eval" />
  <chart y="sft_loss" ghost="parent" phase="sft" />
  <stats of="core val_bpb" vs="final" />
</view>
```

### An eval by category

The run reports `run.row("categories", category=c, accuracy=acc)` as each
category finishes and `run.step(done, accuracy=overall)` per task.

```html
<signal name="category" label="Category" />
<signal name="accuracy" label="Accuracy" digits="3" better="higher" />
<view headline="accuracy">
  <chart from="categories" x="category" y="accuracy" mark="bars" ghost="parent" />
  <stats of="accuracy" />
  <table name="categories" columns="category accuracy" vs="final" />
</view>
```

### A speed and size frontier against a reference built in the run

A codec benchmark builds a reference and the candidate, then times both at
nine levels: its plan is `build`, then `bench` of 9 levels. Each level is one
row with both.

```python
for done, level in enumerate(levels, 1):
    ref, cand = bench(reference, level), bench(candidate, level)
    run.row("levels", level=level, ref_ms=ref.ms, ref_kb=ref.kb, cand_ms=cand.ms, cand_kb=cand.kb)
    run.step(done)
run.step(done, front_gain=gain, valid=int(bit_exact))
```

```html
<signal name="front_gain" label="Frontier gain" digits="4" better="higher" sign="true" />
<signal name="valid" label="Bit-exact" labels='{"1":"valid","0":"invalid"}' />
<signal name="cand_kb" label="Size" unit="KB" better="lower" />
<signal name="cand_ms" label="Compress time" unit="ms" better="lower" />
<view headline="front_gain">
  <chart from="levels" x-scale="log" ghost="parent" title="Size against compress time">
    <series x="ref_ms" y="ref_kb" label="Reference" ghost="false" />
    <series x="cand_ms" y="cand_kb" label="Candidate" />
  </chart>
  <stats of="front_gain valid" vs="final" />
  <table name="levels" columns="level cand_kb cand_ms ref_kb ref_ms" vs="final" />
</view>
```

### A sweep inside one run

One run trains eight learning rates briefly, then refits the best. Each rate
is a step of `sweep` and a row; its curve is rows too, one series per rate.

```json
{"plan": [{"phase": "sweep", "of": 8, "unit": "rates"}, {"phase": "refit", "of": 2000, "unit": "steps"}]}
```

```python
best = float("inf")
for done, lr in enumerate(rates, 1):
    for step, loss in train(lr, steps=200):
        if step % 20 == 0:
            run.row("curves", step=step, lr=lr, loss=loss)
    val = evaluate()
    best = min(best, val)
    run.row("points", lr=lr, val_loss=val)
    run.step(done, best_val_loss=best)
with run.phase("refit", total=2000, unit="steps") as p:
    for step, loss in train(best_lr, steps=2000):
        p.step(step, loss=loss)
    p.step(step, val_loss=evaluate())
```

```html
<signal name="val_loss" label="Val loss" digits="3" better="lower" />
<signal name="best_val_loss" label="Best sweep val loss" digits="3" better="lower" />
<signal name="loss" label="Loss" digits="3" better="lower" />
<view headline="val_loss best_val_loss">
  <chart from="curves" x="step" y="loss" by="lr" phase="sweep" title="Loss by learning rate" />
  <chart from="points" x="lr" y="val_loss" x-scale="log" mark="points" ghost="parent" />
  <chart y="loss" phase="refit" ghost="parent" />
</view>
```

### A frozen solver read by patterns

The solver binary is the harness: every experiment changes its input deck,
never its code. Its log reads `Step   120 of  2000  t=0.0600  residual=3.21e-05`
and ends with `METRIC l2_error 1.37e-04`.

```json
[{"regex": "^Reading mesh block (?P<step>\\d+)/(?P<of>\\d+)", "phase": "mesh", "unit": "blocks"},
 {"regex": "^Step\\s+(?P<step>\\d+) of\\s+(?P<of>\\d+) .*residual=(?P<residual>\\S+)", "phase": "solve", "unit": "steps"},
 {"regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$"}]
```

```json
{"plan": [{"phase": "mesh"}, {"phase": "solve"}]}
```

```html
<signal name="l2_error" label="L2 error" digits="3" better="lower" />
<signal name="residual" label="Residual" digits="2" better="lower" />
<view headline="l2_error residual">
  <chart y="residual" y-scale="log" ghost="parent" />
  <stats of="l2_error residual" vs="final" />
</view>
```

```sh
orx exp view set <expId> --pattern patterns.json --config plan.json --file view.xml
```
