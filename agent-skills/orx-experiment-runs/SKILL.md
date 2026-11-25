---
name: orx-experiment-runs
description: "Make a run's progress, measures and view readable. Use before a baseline's first launch, when a flag wakes you, or when a run's progress or chart misleads."
---

Before a baseline's first launch, follow steps 1–4 in order; after every
launch, step 5. Children inherit the run command, view, patterns, plan and
rules. Keep the command fixed. When a child's code or configuration changes its
workload, update its stage plan to match; before sizing a child, read its
parent's measured runtime with `orx exp status <parentExpId>`.

If a reference beside this skill can't be read,
`orx skill experiment-runs/<path>` prints it, with the path as written here
(`references/examples/sweep.md`) or shortened (`view`, `examples/sweep`).

## 1. Read the entry point

Read the script the run command starts, end to end, with what it calls. List
its stages in order, setup included (install, download, preprocess, train,
eval, export), each with its amount of work (steps, files, tasks) and whether
that number is known now or only once the run computes it. A stage is a wait
the user sits through; an eval every 500 steps inside training is a measure of
`train`.

## 2. Declare the plan

Declare every stage so the bar spans the whole run from its first second,
instead of filling up before eval starts. Save the totals you know in
`plan.json` and set it with `orx exp view set <expId> --config plan.json`:

```json
{"plan": [{"phase": "install"},
          {"phase": "download", "of": 9, "unit": "shards"},
          {"phase": "train", "of": 5000, "unit": "steps"},
          {"phase": "eval", "of": 22, "unit": "tasks"}]}
```

When only the run knows a total (iterations from a FLOP budget, files found on
disk), omit it from `plan.json`; the run declares the plan itself once it
knows, before the first stage starts, and its plan replaces the experiment's
for that run:

```python
orx.plan(["install", {"phase": "train", "of": iterations, "unit": "steps"}, "eval"])
```

A stage that recurs is listed again where it recurs, never twice in a row.

- `references/timing.md` — when the user names a duration or a budget, or a stage shows as unplanned, skipped or untimed

## 3. Report progress

Add the reports to the baseline's code and commit them before its first
launch. A run reports with `::orx::` lines on its output:

```text
::orx:: {"phase":"download","step":3,"of":9,"unit":"shards"}
::orx:: {"step":1240,"of":5000,"t":1824.5,"m":{"loss":2.31,"val_bpb":1.17}}
::orx:: {"row":"tasks","m":{"task":"arc_easy","accuracy":0.62}}
```

From Python, vendor the stdlib-only helper and commit it with the code:

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

- `references/progress.md` — to print `::orx::` lines without the helper (each field's meaning, shell scripts), before a multi-process launch (torchrun, mpirun), to resume from a checkpoint, or when the helper warns on stderr
- `references/patterns.md` — when the measuring code is frozen (a shared harness, a vendored tool) and only its log can be read

## 4. Write the view, and name what would go wrong

Every baseline gets a view before its first launch: the measures that answer
the question and the charts that draw them.

When you can name a failure before launch (a learning rate that may diverge,
a label or data change that may break training, an eval that may print zeros),
say it briefly: a rule if you would stop on it, else one `worth_a_flag` line in
the watcher's brief, the only way the watcher can tell a broken run from an odd
one.

- `references/view.md` — before writing or changing a run's view; it lists worked examples
- `references/rules.md` — to stop a run automatically on a failure you would stop on sight, or when you stop a run yourself
- `references/watcher.md` — to write the watcher's brief, or when a flag wakes you

## 5. Check after every launch

Before replying to the user:

```sh
orx exp view check <runId>
```

It names missing reports, unmatched patterns and disagreements with the plan;
if it has seen nothing yet, check once more on your next turn after setup. On
your next turn after first data arrives, and before answering any message
about the card, a chart, the line or what the user sees, read what the card
collected instead of guessing from the view you wrote:

```sh
orx exp view show <runId>
```

Repair the view, plan or patterns now with `orx exp view set`: the changes
apply to the run in flight, so never defer a view fix to the next relaunch.
Then run `view show` again to confirm.

Then end your turn, whether or not the check settled anything: the result
arrives as a message when the run ends, and the watcher flags trouble while it
runs. Never wait inside a turn (no `sleep` loops over `orx runs` or
`orx exp status`, no `orx exp wait`): a waiting turn holds back the user's next
message and every other run's result, and outlives a short run anyway.

- `references/check.md` — when a missing report needs a code change, when orx warns about reporting at start-up, or for what `view check` and `view show` cover
- `references/live-changes.md` — when the user asks mid-run for another chart or comparison, or the view misleads
- `references/figure-citation.md` — when a message reports a number the run's view shows
- `references/status-json.md` — when a script reads a run's status
