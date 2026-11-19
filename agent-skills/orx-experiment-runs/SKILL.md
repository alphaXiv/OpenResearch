---
name: orx-experiment-runs
description: "Make a run followable from launch to result: declare its stages, report progress and the measures that answer its question, write its view against the parent, read a log you can't edit with patterns, brief the watcher and settle its flags. Use before a baseline's first launch, when a run's progress or chart misleads, when a flag wakes you, and when the user asks to change what a run shows."
---

Before launching, follow sections 1–6 in order; after launching, follow section 7.
Children inherit the run command, view, patterns, plan and rules. Keep the
command fixed. When a child's code or configuration changes its workload,
update its stage plan to match. Use sections 8–10 when changing a live view
or handling rules and flags.

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
The bar times each stage by the experiment's own last run of it, else the
parent's, scaled to this run's total; a run that counted the stage in another
unit, or where only one of the two declared a total, times nothing, and the
stage stays untimed. A stage the run reaches outside the plan shows as unplanned, and
a planned stage it passes over, or finishes without, shows as skipped: both
mean the plan is wrong, so fix it. A cancelled or failed run's later stages
are only unreached.

Save the plan in `plan.json`, omitting totals that only the run can know:

```sh
orx exp view set <expId> --config plan.json
```

When the user names a duration, run:

```sh
orx exp run <expId> --estimate
```

If the estimate falls below the user's requested window, add steps or data or
run a short calibration before launching, and tell the user which you did.
Resize the workload in the child's code or configuration and update its
stage plan. Re-estimate until the total falls inside the user's range.
If the estimate says "at least," report the floor and calibrate the untimed
stages before promising a duration. Before launching, give the estimate and
its hardware assumption in one line.

## 3. Report progress and measures

Add the reports to the baseline's code and commit them before its first
launch. A run reports with `::orx::` lines on its output:

```text
::orx:: {"phase":"download","step":3,"of":9,"unit":"shards"}
::orx:: {"step":1240,"of":5000,"t":1824.5,"m":{"loss":2.31,"val_bpb":1.17}}
::orx:: {"row":"tasks","m":{"task":"arc_easy","accuracy":0.62}}
```

`phase` starts a stage with no step, total or unit until a report gives them;
within a stage, omitted fields keep their last value. `step` is
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

Only rank zero reports. The helper takes the rank from `rank=`, else `RANK` or
`SLURM_PROCID`; under a launcher that sets neither, pass `rank=`, or every
process reports and each row counts once per process. It flushes a non-finite
measure at once, says on stderr when orx will ignore something it printed (a
boolean, a list, a name like `val/loss`), and never marks a run finished.
Resume with `progress(total=5000, start=3000)`.
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
| `view` | `headline="a b"`: the first with a number is shown; a signal that went wrong (not finite, or zero where the parent's is not) leads instead, wherever it is listed |
| `chart` | `y="a b"`, `ghost="parent\|false"` (default `parent`), `x-range="run\|parent"`, `x-scale="log"`, `y-scale="log"`, `mark="line\|points\|bars"`; with `from="table"`, `x`/`y` are its columns, `by="column"` draws a series per value, and `<series x= y= label= ghost=/>` children give each series its own columns |
| `stats` | `of="a b"`, `vs` |
| `table` | `name`, `columns="a b"`, `last="5"`, `vs="parent\|step\|final"` |
| `text` | Plain text with optional `{signal}` tokens and `**bold**`, up to 2,048 bytes |
| `cells` | `from="table"`, newest 256 retained outcome rows in completion order |
| `bars` | `from="table"`, `by="column"`, correct share by category over retained outcome rows |
| `media` | `glob="samples/*.png"`, `last="6"` (1–6, default 6) |

A chart over signals uses reported steps when available and elapsed time otherwise.
Users can change `x-scale`, `y-scale` and `smooth` from a chart's menu, so read
the current view with `orx exp view get` and keep those choices when you run
`orx exp view set`.
Every block also takes `phase` and `title`. The strip only picks a phased chart
during its stage. The run page and embedded figures can show blocks from any stage.
Declare a `signal` for every measure the view names and every column the user
should read by a label. One chart answers one question: measures on different
scales get a chart each. A chart draws at most 8 series and a stats block
takes up to 32 names; show about three stats, the headline first. The strip
shows the headline and the first active chart with two points. A result citation draws the view's first block (`orx-figures`).
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
  ended with. A signal's `final` comparison is available once both runs end.
  Left out, it is `step` while the run goes and `final` once it ends. Use
  `final` when the parent's result is the bar to clear, `step` when
  the question is the path (does it learn faster?). A table compares only with
  `vs`; `vs="parent"` takes the default.
- **`ghost="parent"`** (the default) draws the parent's same series dashed
  under each line. Put `ghost="false"` on a chart to hide its parent curves, or
  on a `<series>` the parent would only repeat, such as a reference measured
  the same way in every run.
- **`x-range`** on a chart over steps: `run` (the default) spans this run's
  total and cuts the parent off at the edge; `parent` spans the parent's run
  too, for a child that stops early or runs longer.

For a local run, use `<media glob="samples/*.png" last="6" />` inside the
view to inspect generated images on the run page and in a result citation when
it is the first block. Put the files under `runs/$ORX_RUN_ID/samples/` in
the project's Artifacts directory on the machine running `orx up`, including
when the workspace itself is remote. Pass that directory to the job explicitly.
Write temporary files, then rename each completed image into place with a
unique name. Use PNG, JPEG or WebP, up to 4 MiB and 4,096 pixels per side.
A view can hold up to four media blocks, each with one `*` in the filename
and literal folders. Previews show the newest matches among up to 2,048
entries per folder. Use `orx exp view check RUN` to check the glob and placement
if no images appear. SSH, Slurm and cloud jobs do not have image previews yet.

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
The check never stops the run. When the same wake brings a result, interpret it
and decide the next run first. A warning matters only when a run still going,
or a planned child that inherits its view, needs the missing report for a
decision. Then inspect `orx exp view show RUN`, `orx exp view check RUN` and
`orx logs RUN` before acting. Card repairs are housekeeping: make them without
reporting them unless the user asked.

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

`view check` names missing reports, unmatched patterns and disagreements with
the plan. With patterns, given or saved, it folds the real log; without, it
reads what collection last saved, which can trail the log for a run's first
seconds. If it has seen nothing yet, check once more on your next turn after
setup; never wait inside a turn for data.

Repair the view, plan or patterns with `view set`: they apply to the run in
flight. If the missing report requires a code change, weigh the work already
done before relaunching. In Agentic mode you may cancel, fix and commit the
code, then relaunch. In Copilot mode, ask the user before cancelling healthy
compute or relaunching. Report actual progress through long validation stages
on later launches so the run shows that work is advancing.

Then go on with work that doesn't depend on this run; when none is left, end
your turn with a `running` `yield`. If the run already finished, read its
result instead.
The result arrives as a message when the run ends, and the watcher flags
trouble while it runs. Never wait inside a turn: no `sleep` loops over
`orx runs` or `orx exp status`, no `orx exp wait`. A turn that waits holds
back the user's next message (such as "chart it differently" while it runs)
and every other run's result, and outlives a short run anyway.

When reporting a result about a number shown in the run's first view block,
cite `<run id="RUN_ID" figure/>` in the message. It shows the first block of
the run's current view, with live data beneath the chip. Editing or reordering
the view can change or remove figures in earlier messages. Use `label="…"` to
name the cited result. Reuse that figure instead of plotting the same data again.
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

A pattern's rows go to the table its `row` names. Rows the code reports under
the same name join that table, so a result both printed and reported counts
twice: give a pattern's table a name the code does not use.

## 9. Rules that stop a run

A run you would stop the moment you saw it, such as a NaN loss or a loss far
above the parent's at the same step, gets a rule, so it stops without waiting
for you. Rules sit beside the plan in the config, and a run keeps the rules it
was launched with. OpenResearch checks them with no model each time it
collects new reports, once over the whole batch: three NaNs and a finite loss
collected together end the streak and stop nothing.
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
  {"name": "diverging", "text": "Stop when the loss is more than 1 above the parent's at the same step, 3 reports running.",
   "does": "stop", "when": {"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower", "reports": 3}},
  {"name": "plateau", "text": "Ask when val loss improves by less than 0.01 over 4 evals.",
   "does": "ask", "when": {"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}}
]}
```

The `kind` field names the condition. For `nonFinite`, `compare`, and
`worseThanParent`, `reports` counts matches in a row and defaults to 1.
`worseThanParent` skips a report with nothing to compare (the parent has no
value there, or either value is not a number): it neither extends nor breaks
the streak. `noImprovement` requires `reports` to set the size of its window.

| `when` | Trips when |
| --- | --- |
| `{"kind": "nonFinite", "signal": "loss"}` | the measure is NaN or infinite |
| `{"kind": "compare", "signal": "eval_score", "op": "lt", "value": 0.1}` | it is below (`lt`, `le`) or above (`gt`, `ge`) a value |
| `{"kind": "worseThanParent", "signal": "loss", "by": 1, "better": "lower"}` | it is worse than the parent's run at the same step by more than `by` |
| `{"kind": "noImprovement", "signal": "val_loss", "min": 0.01, "reports": 4, "better": "lower"}` | its best of the last `reports` beats the best before them by less than `min` |
| `{"kind": "noProgress", "seconds": 600}` | no step was reported for that long |
| `{"kind": "costAbove", "usd": 5}` | spend reached it, or the estimate says it will pass it |
| `{"kind": "projectedFinishPastLimit"}` | the estimate says the run ends after its time limit |

The parent's "same step" is its value in the same stage at that step, or at
the same elapsed time when the run reports no steps; between two of the
parent's reports it is read off the line joining them.

`projectedFinishPastLimit` requires a run with a time limit. OpenResearch,
Hugging Face Jobs, Modal, Kubernetes, and Vercel supply one by default. For
Slurm, pass `--timeout` or set a time limit in the Slurm settings. Local, SSH,
Ray, and Tinker launches with this rule are refused.

`"stop"` cancels the run, which shows as Auto-stopped by that rule, and you
are woken with what orx saw; `"ask"` raises a flag (§10) and the run goes on. `text` is
what the user reads, so say the condition in your words. Rules act only while
`orx up` is running and the workspace machine is awake.

When you stop a run yourself, say why: `orx exp cancel --run <runId> --reason
'…'`. If the stopped run answered nothing and you are about to repair the same
experiment and run it again, first `orx exp revise <expId> --run <runId>`, so
the user sees Revising until the next run starts.

## 10. What the watcher is told, and its flags

A cheap model looks at each run while it goes and writes the sentence on its
line. It sees a digest: the run's numbers, its newest log lines and a brief
you write; never the chat, the repository or the whole log. Write the brief beside
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
while output keeps coming, at least every `maxGapS` seconds. `minGapS` keeps
the looks changes bring apart; a look that ends a silence can come sooner.
Leave `watch` out for the defaults (2 and 30 minutes); widen the
gaps for a run of days, narrow `maxGapS` for one that may go wrong fast.

A flag is the watcher, a rule that asks, or orx's own checks (a value that is
not a number, a measure half again worse than the parent's at the same step, no
progress, a new kind of error) saying a run may be going
wrong. It never stops the run. Unless the chat is Collaborative, it wakes you
with the lines it saw and where they are: it saw a digest and none of the chat, so read the run
yourself (`orx exp status <expId> --run <runId>`, `orx logs <runId>`) and
confirm before acting. If you own the choice, settle it before ending your
turn, with a reason the user reads:

```sh
orx exp flag <runId> --flag <flagId> --keep --reason 'The spike is the eval, as the brief says.'
orx exp flag <runId> --flag <flagId> --stop --reason 'Loss is NaN since step 410; lowering the learning rate.'
orx exp flag <runId> --flag <flagId> --ask --reason 'It may be fine; the user knows the dataset.'
```

`--keep` settles this flag. Later looks read what you kept and why. Say what you
verified and what evidence would change your mind. A later look can raise a new flag.
`--stop` stops it as `orx exp cancel`
does. `--ask` leaves the choice to the user, who sees Stop run, Keep going and
Ask in the chat. In a Collaborative chat the flag waits for the user, and you
hear of it only when they ask you. The reason shows on the run's card, so tell
the user about a flag in chat only when it changes a finding or the next run,
when you `--ask`, or when they asked about it.

For recurring model flags, read `orx exp view get EXP --json`. Write the revised
`brief` object to a JSON file as above, preserving its other labels. Use `noise`
for expected behavior and `worth_a_flag` for evidence that needs investigation.
To space out change-triggered looks, increase `watch.minGapS` and keep the
desired `watch.maxGapS`. Save with `orx exp view set EXP --config PATH`. The
changes apply to the next look built for this run.

For a built-in stall during healthy validation, have the code report a
validation phase and advancing steps on the next launch. For a flag from a
rule, find the named rule in the experiment's config and revise it there.
Rule edits apply to later launches.

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
