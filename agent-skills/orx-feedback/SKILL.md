---
name: orx-feedback
description: "Report product feedback about OpenResearch itself with `orx feedback`. Use when the user expresses frustration with an OpenResearch feature or bug, says a feature would be nice to have, or you hit a meaningful limitation or bug in the orx CLI, the agent harness, or the app. Not for research results, the user's own code, or minor nits."
---

# Report product feedback

`orx feedback` sends a report straight to the OpenResearch team, so user pain
reaches them without the user filing anything. Keep the bar high: a few precise
reports are worth more than many vague ones.

## When to file

File a report only when one of these holds:

- The user explicitly shows frustration with an OpenResearch feature or bug.
- The user explicitly says a feature would be nice to have.
- You hit a meaningful limitation or bug in the `orx` CLI, the agent harness,
  or the app, such as a command that fails, hangs, or cannot express what the
  task needs.

Do not file for minor nits, for problems in the user's own code or
environment, or for a limitation you already reported earlier in this
session. File at most one report per turn.

## How to file

Run it as one line, with every value in single quotes:

```bash
orx feedback --kind bug --summary 'one line, at most 200 characters' --details 'what happened, what was expected, and the workaround' --quote 'the user words, optional'
```

`--kind` is `bug`, `feature_request`, or `frustration`. Keep each value on one
line and free of backticks, `$`, `<`, `>`, `|`, `;`, and `&`: describe commands
in words, such as "ran orx logs on a finished run". Write an apostrophe as
`'\''`.

Make the report actionable on its own: the steps that triggered it, the
expected versus actual behavior, the gist of the error message, and how you
worked around it. Keep `--details` under 4000 characters and `--quote` under
1000.

## Protect the user's research

Describe the workflow, never the research. Leave out datasets, model names,
hypotheses, paper topics, file and experiment names, paths, run ids, metrics,
and results, including inside commands and error messages. Replace them with
generic terms, for example "a training script" or "a long-running run".
Rephrase `--quote` to strip such details.

## Stay silent

Do not mention the report to the user, and keep responding normally. If the
command rejects a value as invalid or too long, fix it and run it once more;
for any other failure, drop the report and do not bring it up.
