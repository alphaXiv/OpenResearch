---
name: orx-agent-delegation
description: "Delegate independent work to helper agent sessions with `orx agent spawn`: task selection, briefs, branch ownership, compute authorization, wakeups, concurrency, and cross-harness review on other vendors' agents. Use before spawning a helper, interpreting its result, or when cross-harness review is on; do not delegate the literature retrieval loop."
---

# Delegate work to another agent

`orx agent spawn` creates a new top-level session in the same project. The
helper is visible to the user, receives its own worktree and transcript, and
works independently from this session.

```sh
orx agent spawn "<self-contained task>"
orx agent spawn --title "<session title>" --stdin
orx agent spawn "<task>" --harness <harness> --model <model>
orx agent spawn "<task>" --no-wake
```

By default, this chat resumes with the helper's closing reply. Use `--no-wake`
only when no follow-up is needed. A spawned session cannot spawn another helper,
and the CLI enforces the number of helpers a session may have in flight. If the
command refuses a spawn for either reason, do the work here or wait for a helper
to finish. It also refuses a `--harness` that OpenResearch cannot find installed;
spawn on this session's harness instead, or tell the user.

## Choose tasks with a clean boundary

Delegate work that is genuinely independent from the node this session owns,
such as surveying an unfamiliar codebase or writing up completed results. Do
the work here when it is a step in the experiment loop already underway.

Never delegate the retrieval loop covered by `orx-lit-review`: the main agent
must inspect and rank the combined literature candidates itself.

## Protect branches and compute

- Never give a helper a branch checked out by this session. If it must change
  experiment code, tell it to create its own node and work on that node's branch.
- A frozen experiment node may not be edited by either session.
- State exactly which `orx exp run` calls the helper may launch. Explicitly
  forbid launches when none are authorized; otherwise the helper may infer that
  the normal research loop is available.
- The helper's edits remain in its worktree. Nothing merges into this branch
  automatically.

## Write a standalone brief

The helper starts with an empty transcript and cannot see this conversation.
Include the project, relevant experiment and branch, metric, constraints,
allowed compute, expected output, and a concrete definition of done. Use
`--stdin` for a multi-paragraph brief.

A Claude helper's closing reply waits for its background commands to exit (or
about 30 quiet minutes), so do not ask it to leave servers or watchers running
unless the task needs them.

## Cross-harness review

When the session playbook says cross-harness review is on, the user wants the
research not to rest on one vendor's agent. Each agent family has its own
habits in what it proposes, how generously it reads results, and how it
writes, so route the steps where those habits can bias conclusions through a
helper on a different vendor's harness:

```sh
orx agent harnesses                     # installed harnesses and their model vendors
orx agent spawn --harness codex --title "Independent check: <claim>" --stdin
```

- **Ideas**: before committing to the next round of experiments, ask one helper
  on another vendor to propose its own candidates from the same evidence, then
  pick from the union and say where each idea came from.
- **Results**: before a claim enters a report or paper, have a helper on another
  vendor re-derive it from `orx logs` and the run outputs without seeing your
  interpretation. Record agreement, or keep the claim provisional and state the
  disagreement.
- **Writing**: have a helper on another vendor review a paper or report draft
  for overclaiming, missing baselines, and statistical errors.

Rules:

- Use only harnesses the user left on for review: the `REVIEW` column of
  `orx agent harnesses` and the playbook's list. Prefer one whose vendor differs
  from this session's; OpenCode and Cursor count by the model they run. If no
  allowed harness from another vendor is installed, say so once and continue
  on this session's harness.
- Give the helper the evidence and the question, not your conclusion. Forbid
  experiment launches unless the user approved them.
- The wake-up names the helper's harness and model. Keep that attribution in
  experiment notes, reports, and the paper's methods or acknowledgements, for
  example "Result independently re-derived by a Codex (OpenAI) agent."
- Do not split the experiment loop itself across harnesses, and never delegate
  the literature retrieval loop.
