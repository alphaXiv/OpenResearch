---
name: orx-experiment-runs
description: "Make a run followable from launch to result: declare its stages, report progress and the measures that answer its question, write its view against the parent, read a log you can't edit with patterns, brief the watcher and settle its flags. Use before a baseline's first launch, when a run's progress or chart misleads, when a flag wakes you, and when the user asks to change what a run shows."
---

Before launching, follow sections 1–6 in order; after launching, follow section 7.
Children inherit the view, patterns, plan and rules, but changed arguments
must still report their actual totals. After removing or adding commands in
a child, update its reported stage plan to match before launch. Use sections
8–10 only when changing a live view or handling rules and flags.

Before sizing a child, read its parent's measured runtime:

```sh
orx exp status <parentExpId>
```

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

Save the plan in `plan.json`, omitting totals that only the run can know:

```sh
orx exp view set <expId> --config plan.json
```

When the user names a duration, run:

```sh
orx exp run <expId> --estimate
```

Resize up as well as down, then re-estimate until the total falls inside the
user's range. Below the lower bound is as wrong as above the upper bound.
Never state a total the estimate did not print; with untimed stages, say the
floor and that the rest is unknown, or calibrate first.

Before launching, say the estimated number and hardware assumption in one line. When any stage is untimed,
tell the user the total is unknown or run a short calibration before promising
a duration.

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
run = orx.progress(total=5000, unit="steps", phase="train")   # names its stage when no shell does
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
change; the Python it starts then calls `progress(total=…)` without `phase`:

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
| `chart` | `y="a b"`, `ghost="parent\|false"` (default `parent`), `x-range="run\|parent"`, `x-scale="log"`, `y-scale="log"`, `mark="line\|points\|bars"`; with `from="table"`, `x`/`y` are its columns, `by="column"` draws a series per value, and `<series x= y= label= ghost=/>` children give each series its own columns |
| `stats` | `of="a b"`, `vs` |
| `table` | `name`, `columns="a b"`, `last="5"`, `vs="parent\|step\|final"` |
| `text` | Plain text with optional `{signal}` tokens and `**bold**`, up to 2,048 bytes |
| `cells` | `from="table"`, newest 256 retained outcome rows in completion order |
| `bars` | `from="table"`, `by="column"`, correct share by category over retained outcome rows |

A chart over signals uses reported steps when available and elapsed time otherwise.
Every block also takes `phase` (shown only during that stage) and `title`.
Declare a `signal` for every measure the view names and every column the user
should read by a label. One chart answers one question: measures on different
scales get a chart each. At most 8 series a chart and three stats, the
headline first. The strip shows the headline and the first active chart with
two points. A result citation draws the view's first block (`orx-figures`).
Use the result message for the interpretation of a particular run.

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

For cells and bars, emit one row per finished item. Duplicate rows each count.

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

How a run meets its parent is yours to choose, per experiment:

- **`vs`** on `stats`, `table`, or a `signal` as its default: `step` compares
  with the parent at the same stage and step, `final` with the value the parent
  ended with. Left out, it is `step` while the run goes and `final` once it
  ends. Use `final` when the parent's result is the bar to clear, `step` when
  the question is the path (does it learn faster?). A table compares only with
  `vs`; `vs="parent"` takes the default.
- **`ghost="parent"`** (the default) draws the parent's same series dashed
  under each line. Put `ghost="false"` on a chart to hide its parent curves, or
  on a `<series>` the parent would only repeat, such as a reference measured
  the same way in every run.
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

OpenResearch also checks reporting as the run starts and sends at most one
warning about missing patterns, headline or chart data, or stage totals.
Inspect the run before acting on it; the check never stops the run.

After every launch, before replying to the user:

```sh
orx exp view check <runId>
```

On your next turn after its first data arrives, inspect the card. For any
user message about the card, a chart, the line or what they see, run this
before answering:

```sh
orx exp view show <runId>
```

Read this instead of guessing from the view you wrote. It describes the
collected stage and plan, watcher and flag, time left, charts and parent
ghosts, stats, and names or patterns not reported yet. It reads the card's
stored data, not the user's screen. Fix missing or unused charts now with
`orx exp view set`; its changes apply while the run continues. Never defer a
view fix to the next relaunch. Then run `view show` again to confirm the change.

`view check` folds the real log and names missing reports, unmatched patterns
and disagreements with the plan. If the log is still empty, check once more
on your next turn after setup; never wait inside a turn for data.

Repair the view, plan or patterns with `view set`: they apply to the run in
flight. A report missing from the code means cancelling, fixing and
relaunching, which is cheap now and costly an hour in.

Then end your turn, whether or not the check settled anything.
The result arrives as a message when the run ends, and the watcher flags
trouble while it runs. Never wait inside a turn: no `sleep` loops over
`orx runs` or `orx exp status`, no `orx exp wait`. A turn that waits holds
back the user's next message (such as "chart it differently" while it runs)
and every other run's result, and outlives a short run anyway.

When reporting a result about a number shown in the run's first view block,
cite `<run id="RUN_ID" figure/>` in the message. It shows the run's own block
beneath its chip and stays live while the run runs. Use `label="…"` to name
the cited result. Reuse that figure instead of plotting the same data again.
A run with no view or no data yet shows only its chip.

For scripts, `orx exp status EXP --run RUN --json` returns `experimentId`,
`runId`, `status`, the current `stage` name, `progress` (`done`, `of`, `unit`,
`whole` as a fraction), `latest` signal values by name, `estimate`, and
`flags` in raise order (`id`, `state`, `text`). The estimate has remaining
milliseconds (`loMs`, `midMs`, `hiMs`), the Unix timestamp in milliseconds
when it was measured (`at`), `pending`, `floor`, and `untimed` stage names.
Unknown fields are null, latest values and flags are empty before data arrives.
Use a flag's `id` with `orx exp flag RUN --flag ID`. A flag in `kept` or `stopped`
state is settled.

## 8. Change it while it runs

The user may ask mid-run to chart something else or to compare differently,
and the run itself may show that the view misleads: a stage outside the plan,
a stat compared with the wrong point of the parent, a curve flattened by one
early spike (`y-scale="log"`). Change it then, on each experiment it concerns
(siblings each hold their own copy):

- `--file`: every open page redraws from the new view.
- `--pattern`: the run's kept log is read again with them.
- `--config`: the bar redraws, for runs that declared no plan of their own.

What the code reports is fixed for that run, but what it prints is not all
lost: a pattern over a line the run already prints (§6) reads it into measures
and rows, the kept log included. Asked mid-run for a chart of numbers the run
prints but does not report, add a pattern and change the running experiment's
view at once, in the same turn; never edit the harness while it runs or wait
for the next run to show it. A measure the run never prints at all needs a
code change, and the next run.

Charts of table research (benchmarks, sweeps, tasks by size):

- **Sizes that span decades** (1k to 1M items, learning rates) get
  `x-scale="log"`, and times or slowdowns across them `y-scale="log"`:
  on linear axes everything below the largest size sits at zero.
- **One line per category**: when rows mix kinds (input distributions, tasks,
  datasets), `by="dist"` draws a series per kind; one line through all of them
  zigzags at each size and means nothing.
- **Two measures of one row** (the reference's time and the candidate's) are
  two `<series>` with labels, `ghost="false"` on the reference.

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

A pattern's rows go to a table of their own, apart from the rows the code
reports, so the two never interleave.

## 9. Rules that stop a run

A run you would stop the moment you saw it, such as a NaN loss or a loss far
above the parent's at the same step, gets a rule, so it stops without waiting
for you. Rules sit beside the plan in the config, OpenResearch checks them on
every report with no model, and a run keeps the rules it was launched with.
Declare only what the task justifies: a stop ends the run, and a wrong rule
costs a rerun.

When a run has a failure you can name before it starts (a learning rate that
may diverge, a change to the labels or data that may break training, an eval
that may print zeros), say it before launch, briefly: a rule if you would stop
on it, else one `worth_a_flag` line in the brief (§10). The watcher knows only
what the digest tells it, so a sentence from you is what lets it tell a broken
run from an odd one.

```json
{"rules": [
  {"name": "nan-loss", "text": "Stop when the loss is NaN on 3 reports in a row.",
   "does": "stop", "when": {"kind": "nonFinite", "signal": "loss", "reports": 3}},
  {"name": "diverging", "text": "Stop when the loss is 1 above the parent's at the same step, 3 reports running.",
   "does": "stop", "when": {"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower", "reports": 3}},
  {"name": "plateau", "text": "Ask when val loss improves by less than 0.01 over 4 evals.",
   "does": "ask", "when": {"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}}
]}
```

The `kind` field names the condition. For `nonFinite`, `compare`, and
`worseThanParent`, `reports` counts matches in a row and defaults to 1.
`noImprovement` requires `reports` to set the size of its window.

| `when` | Trips when |
| --- | --- |
| `{"kind": "nonFinite", "signal": "loss"}` | the measure is NaN or infinite |
| `{"kind": "compare", "signal": "eval_score", "op": "lt", "value": 0.1}` | it is below (`lt`, `le`) or above (`gt`, `ge`) a value |
| `{"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower"}` | it is worse than the parent's run at the same step by more than `by` |
| `{"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}` | its best of the last `reports` beats the best before them by less than `min` |
| `{"kind": "noProgress", "seconds": 600}` | no step was reported for that long |
| `{"kind": "costAbove", "usd": 5}` | spend passed it, or the estimate says it will |
| `{"kind": "projectedFinishPastLimit"}` | the estimate says the run ends after its time limit |

`"stop"` cancels the run, which shows as Auto-stopped by that rule, and you
are woken with what orx saw; `"ask"` raises a flag (§10) and the run goes on. `text` is
what the user reads, so say the condition in your words. `orx exp run <expId>
--budget 5` adds a `budget` rule to that run.

When you stop a run yourself, say why: `orx exp cancel --run <runId> --reason
'…'`. If the stopped run answered nothing and you are about to repair the same
experiment and run it again, first `orx exp revise <expId> --run <runId>`, so
the user sees Revising until the next run starts.

## 10. What the watcher is told, and its flags

A cheap model looks at each run while it goes and writes the sentence on its
line. It sees a digest: the run's numbers, its newest log lines and a brief
you write; never the chat, the code or the whole log. Write the brief beside
the plan in the config, in plain sentences, by label; children inherit it:

```json
{"brief": {
  "question": "Does depth 8 beat depth 4 on val bpb at the same token budget?",
  "healthy": "Loss falls fast in the first 200 steps, then slowly; val bpb below the parent's by step 2000.",
  "noise": "A tokens/s dip at every eval is the eval itself. Warnings from torch.compile at start are expected.",
  "worth_a_flag": "Any line that says depth=4: this run must use depth 8.",
  "from_parent": "The parent's val bpb flattened at 1.21 after step 4000.",
  "decided": "Warm-up is 200 steps; a loss spike before step 200 is not a problem."
 },
 "watch": {"minGapS": 120, "maxGapS": 1800}}
```

`worth_a_flag` is where you name what you would want to hear about; the more
specific, the fewer false flags. A look comes when something changes (a
number leaves its trend, a new kind of warning, a stage starts or ends) and,
while output keeps coming, at least every `maxGapS` seconds; `minGapS` keeps
looks apart. Leave `watch` out for the defaults (2 and 30 minutes); widen the
gaps for a run of days, narrow `maxGapS` for one that may go wrong fast.

A flag is the watcher, a rule that asks, or orx's own checks (a value that is
not a number, a measure half again worse than the parent's at the same step, no
progress, a new kind of error) saying a run may be going
wrong. It never stops the run. In Agentic mode it wakes you with the lines it
saw and where they are: it saw a digest and none of the chat, so read the run
yourself (`orx exp status <expId> --run <runId>`, `orx logs <runId>`) and
confirm before acting. Then settle it, with a reason the user reads:

```sh
orx exp flag <runId> --flag <flagId> --keep --reason 'The spike is the eval, as the brief says.'
orx exp flag <runId> --flag <flagId> --stop --reason 'Loss is NaN since step 410; lowering the learning rate.'
orx exp flag <runId> --flag <flagId> --ask --reason 'It may be fine; the user knows the dataset.'
```

`--keep` lets it run, and the watcher's next looks read what you kept and why,
so a warning you judged harmless is not raised again on that run; add it to the brief's
`noise` for the experiment's next runs. `--stop` stops it as `orx exp cancel`
does. `--ask` leaves the choice to the user, who sees Stop run, Keep going and
Ask in the chat. In Copilot mode the flag waits for the user, and you hear of it
only when they ask you.

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
