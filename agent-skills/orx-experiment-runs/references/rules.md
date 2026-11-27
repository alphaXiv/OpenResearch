# Rules that stop a run

A run you would stop the moment you saw it, such as a NaN loss or a loss far
above the parent's at the same step, gets a rule, so it stops without waiting
for you. Rules sit beside the plan in the config, and a run keeps the rules it
was launched with. OpenResearch checks them with no model each time it
collects new reports, once over the whole batch: three NaNs and a finite loss
collected together end the streak and stop nothing. Declare only what the task
justifies: a stop ends the run, and a wrong rule costs a rerun.

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

```sh
orx exp view set <expId> --config plan.json
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
Hugging Face Jobs, Modal, Kubernetes, Vercel, and boxd supply one by default. For
Slurm, pass `--timeout` or set a time limit in the Slurm settings. Local, SSH,
Ray, and Tinker launches with this rule are refused.

`"stop"` cancels the run, which shows as Auto-stopped by that rule, and you
are woken with what orx saw; `"ask"` raises a flag (`references/watcher.md`)
and the run goes on. `text` is what the user reads, so say the condition in
your words. Rules act only while `orx up` is running and the workspace machine
is awake.

## Stopping a run yourself

When you stop a run yourself, say why:
`orx exp cancel --run <runId> --reason '…'`. If the stopped run answered
nothing and you are about to repair the same experiment and run it again, first
`orx exp revise <expId> --run <runId>`, so the user sees Revising until the
next run starts.
