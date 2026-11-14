# Instrument a baseline

A run the user can follow without asking you says three things: which stage it
is in and how far along, the numbers that answer its question, and how those
compare with its parent. Write that once, on the baseline, before its first
run. Children inherit the code, the view and the patterns.

## 1. Every stage reports

Read the entry point the run command starts, end to end, and list its stages,
setup included: installing, downloading, preprocessing, training, evaluating.
Each becomes a phase with its own total and unit, reported as it goes. A stage
that reports nothing looks stalled for as long as it runs.

A shell script names the phases and relays the progress of tools it cannot
change:

```sh
report() { printf '::orx:: %s\n' "$1"; }
report '{"phase":"download","step":0,"of":9,"unit":"shards"}'
python download.py | while IFS= read -r line; do
    printf '%s\n' "$line"
    case $line in "Saved shard"*) n=$((n + 1)); report "{\"step\":$n}" ;; esac
done
report '{"phase":"train"}'
python train.py
```

The Python inside a phase reports its steps and measures with `orx_run`
(`orx-compute`, "Report progress and measurements"):

```python
run = orx_run.progress(total=cfg.steps, unit="steps")   # right after its phase starts
run.step(step, loss=loss, tok_per_sec=tps)               # every step; the helper rate-limits
run.step(step, val_bpb=val)                              # each eval, at the step it measured
```

Set `PYTHONUNBUFFERED=1` when output passes through a pipe, or lines arrive
late and out of order.

## 2. The measures that answer the question

- **The headline is the number in the first sentence of the result.** If you
  train on loss and judge by accuracy, the headline is accuracy. If it only
  exists late, list a stand-in after it: `headline="accuracy loss"`.
- **Declare `better`** on every measure that has a direction. Without it there
  is no comparison with the parent.
- **One name, one series, the whole run.** A later stage measuring the same
  thing names it apart: `sft_loss`, not `loss` again.
- **Never rename a measure once children exist.** Change its `label`.
- Results that arrive one unit at a time (tasks, rounds, operating points) are
  rows: `run.row("tasks", task=name, accuracy=acc)`.

## 3. The view

Write markup to a file and set it on the baseline:

```sh
orx exp view set <expId> --file view.xml
orx exp view get <expId>
```

The strip above the chat box shows the headline and the first block for the
stage the run is in: put first the picture that would make you stop the run.

| Element | Attributes |
| --- | --- |
| `signal` | `name`, `label`, `unit`, `digits` (0–12), `better="lower\|higher"`, `smooth` (above 0, at most 1; noisy curves only), `as="duration\|compact"`, `sign="true"`, `pick="latest\|best"` (`best` needs `better`), `labels='{"1":"valid","0":"invalid"}'` |
| `view` | `headline="a b"`: the first with a number is shown |
| `chart` | `y="a b"`, `x` (`step`, the default, or `elapsed`), `ghost="parent"`, `x-scale="log"`, `y-scale="log"`, `mark="line\|points\|bars"`; with `from="table"`, `x`/`y` are its columns, `by="column"` draws one series per value, and `<series x= y= label=/>` children give each series its own x |
| `stats` | `of="a b"` |
| `table` | `name`, `columns="a b"`, `last="5"`, `vs="parent"` |

Declare a `signal` for every measure the view names: once a run has reported,
a name nothing declared or reported is refused. Every block also takes `phase` (shown only during
that stage) and `title`. At most 8 series per chart; three stats, the headline
first. No colours, sizes or HTML: what a block cannot say goes in the result
message (`orx-figures`).

### A training curve with an eval

```html
<signal name="val_bpb" label="Val bpb" digits="3" better="lower" />
<signal name="loss" label="Train loss" digits="3" smooth="0.3" better="lower" />
<signal name="tok_per_sec" label="Tokens/s" as="compact" better="higher" />
<view headline="val_bpb">
  <chart y="val_bpb" ghost="parent" phase="train" />
  <chart y="loss" phase="train" />
  <stats of="val_bpb loss tok_per_sec" />
</view>
```

### An eval with categories

The run reports `run.row("categories", category=c, accuracy=acc)` as each
category finishes and `run.step(done, accuracy=overall)` per task.

```html
<signal name="accuracy" label="Accuracy" digits="3" better="higher" />
<view headline="accuracy">
  <chart from="categories" x="category" y="accuracy" mark="bars" ghost="parent" />
  <stats of="accuracy" />
  <table name="categories" columns="category accuracy" vs="parent" />
</view>
```

### A frontier against a reference measured in the run

One row per operating point, the reference beside the candidate.

```html
<signal name="front_gain" label="Frontier gain" better="higher" sign="true" />
<signal name="valid" label="Bit-exact" labels='{"1":"valid","0":"invalid"}' />
<signal name="cand_kb" label="Size" unit="KB" better="lower" />
<signal name="cand_ms" label="Compress time" unit="ms" better="lower" />
<view headline="front_gain">
  <chart from="levels" x-scale="log" title="Size against compress time">
    <series x="ref_ms" y="ref_kb" label="Reference" />
    <series x="cand_ms" y="cand_kb" label="Candidate" />
  </chart>
  <stats of="front_gain valid" />
  <table name="levels" columns="level speedup size_ratio cand_ms cand_kb" />
</view>
```

## 4. Code that is not yours to edit

When the harness is frozen (it measures every experiment the same way and no
experiment may change it), read its log with patterns instead: a JSON list,
one object per kind of line, set beside the view.

```json
[{ "regex": "^chunk (?P<step>\\d+)/(?P<of>\\d+) done", "phase": "compress", "unit": "chunks" },
 { "regex": "^== reference self-check", "phase": "score" },
 { "regex": "^\\s*(?P<level>\\d)\\s+(?P<ref_ms>[\\d.]+)\\s+(?P<cand_ms>[\\d.]+)\\s+(?P<speedup>[\\d.]+)\\s+(?P<ref_kb>[\\d.]+)\\s+(?P<cand_kb>[\\d.]+)\\s+(?P<size_ratio>[\\d.]+)$", "row": "levels" },
 { "regex": "^METRIC (?P<name>\\w+) (?P<value>\\S+)$" }]
```

```sh
orx exp view set <expId> --file view.xml --pattern patterns.json
```

Named captures `step`, `of`, `t` and `spend` mean what they do on an `::orx::`
line; `name` and `value` make a measure named by the log; any other capture is
a measure, or a cell when the pattern has `row`. `phase`, `unit` and `row` are
captured or fixed per pattern. Rust regex syntax, no lookaround or
backreferences; the first pattern that matches a line reads it. Prefer `::orx::` lines
wherever you own the code: a pattern breaks when the log's wording changes.

## 5. Check it on the real log

After the baseline's first launch, stay until its log has a few minutes of
output, then:

```sh
orx exp view check <runId>
orx exp status <expId>
```

`check` folds the run's actual log: the stages it read, the measures and tables
the run reported, what each pattern matched with its first line, and what the
view still waits for. Fix the view or patterns with `view set`; they
apply to the run in flight. A report missing from the code means cancelling,
fixing and relaunching: cheap now, which is why you check in the first minutes.

## Titles

A node's title is what the user reads beside its progress, among its siblings.
Keep it short with the setting that varies first: "Depth 4 at 2e15 FLOPs",
"LR 3e-4", not "Experiment: trying a deeper model".
