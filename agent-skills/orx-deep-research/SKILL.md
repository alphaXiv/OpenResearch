---
name: orx-deep-research
description: "Coordinate multi-source research and a substantial cited report with independent researchers, cross-harness review, and a report writer. Use for deep research, broad comparisons, or literature synthesis requiring a durable report; use orx-lit-review for focused scientific explanations and quick lookups."
---

# Coordinating deep research

Act as the coordinator: define the question, partition the work, track evidence
coverage, and deliver a verified narrative report in the user's language. In
an OpenResearch session, use `orx agent spawn` for independent researchers and
one writer. Prefer multiple available harnesses and model vendors; do not assume
that a particular CLI, model, provider, or native Agent/Task tool is available.

This workflow permits delegation of distinct literature subtopics. It is the
scoped exception to `orx-lit-review`'s normal single-agent retrieval loop, not
permission to delegate routine lookups or split an active experiment loop.

## Scope and output ownership

Clarify only missing details that materially change the research: ambiguous
entities, time window, geography, decision criteria, or requested depth. State
reasonable assumptions and proceed when the answer can still be useful. A
simple factual question does not need this workflow.

Choose a specific title in the user's language, normally 3–6 words and fewer
than 50 characters, using letters, numbers and spaces. Preserve accents and
non-Latin writing. Inspect existing output names and add a distinguishing topic
or number for follow-ups. Never overwrite earlier research.

Use the absolute artifacts directory in the session playbook. Inside a topic
folder, create `research_notes/<title>/` and `reports/`; the report is
`reports/<title>.md`. Outside OpenResearch, use the current working directory
(or the user's chosen destination), not a temporary/uploads folder. Record
absolute paths; helpers have separate worktrees, so relative paths are unsafe.
For related prior research, pass the earlier report and notes as background;
for an independent redo, withhold them. Keep each request's notes separate.

The coordinator may inspect project metadata, create directories, write task
briefs and a compact progress manifest, and verify outputs. Delegate evidence
collection and report drafting to keep context focused. Do not load the two
role references into the coordinator's context unless doing that role because
no helper mechanism is available. Never interpret this separation as a ban on
reading the final report or verifying its claims.

## Harness selection and bounded parallel work

Read `orx-agent-delegation` for the installed command contract, then run:

```sh
orx agent harnesses --json
```

Use `installed: true` harnesses; `installed: null` means unknown, not available.
For independent review, honor `review: true` and the user's permitted model
choices. Prefer a vendor different from the coordinator's when one is allowed;
OpenCode and Cursor take their vendor from the selected model, not their name.
Do not install harnesses, change credentials, enable disabled review targets,
or invent model identifiers. If only one usable harness exists, state that
constraint once and continue. Do not claim independent vendors without evidence.

Assign 2–3 complementary subtopics for a typical broad request, fewer when one
researcher suffices. Separate primary evidence, opposing evidence/limitations,
and practical implications where appropriate. Use more only for real coverage
needs; OpenResearch permits at most five live helpers per parent and forbids
helpers spawning helpers. Queue larger coverage in batches. User-specified
budgets and tool concurrency limits take precedence.

Launch independent assignments before waiting for completion. OpenResearch
spawns are asynchronous: keep the returned session ids and use the normal
completion wakeups. A successful spawn means accepted, not research completed.
Use the actual harness id returned by discovery, for example:

```sh
orx agent spawn --harness codex --title "Evidence on the main claim" --stdin <<'TASK'
Research the assigned subtopic only.
First read: orx skill deep-research/references/researcher.md
Objective and key questions: [complete self-contained assignment]
Constraints: [scope, dates, jurisdiction, language, available sources, budget]
Project and relevant evidence: [absolute paths and run/experiment identifiers]
Save notes to: [absolute unique topic notes path]
Do not launch experiments, provision compute, publish, or modify experiment branches.
Return the notes path, key findings, unresolved gaps, and actual harness/model.
TASK
```

Fill every bracketed field before use. Quote paths and use a quoted heredoc so
prompt text is not shell code. Give each helper one output filename and no
shared file to edit. Researchers load [references/researcher.md](references/researcher.md),
available through `orx skill deep-research/references/researcher.md` when the
installed skill path is unknown. A child must do its own task and report gaps;
only the coordinator dispatches more work.

Outside an OpenResearch session, use a real native spawn tool if available,
with the same bounded briefs and absolute file ownership. Do not call
`orx agent spawn` outside its supported session context. If no delegation tool
works, research and write directly using the role references and disclose the
single-agent fallback; do not block delivery or fabricate helper outputs.

## Evidence convergence and writing

Track assigned topics, actual harness/model, completion status, files, and gaps
in the request's manifest. Give brief progress updates about research coverage.
Researchers use enabled `orx discover` / `orx paper` primitives for scholarly
work, original web sources for other domains, and local run evidence when
claims concern this project. Do not ask multiple helpers to repeat the same
search loop without a defined independent perspective.

After initial completion, compare the findings and source coverage. Permit at
most one additional round for critical missing evidence or an independent
check of consequential disputed claims. When using it, say: "After these
researchers finish, I will move directly to coordinating the report writer."
Then move to writing, recording any remaining gaps rather than restarting the
research indefinitely. A failed helper can be replaced within this round or
its missing scope explicitly marked; never pass a missing file as evidence.

Spawn one writer, preferably on a different allowed harness from the main
evidence collector. Its complete brief must include:

- The full user question, constraints, language and desired depth.
- Absolute paths of current notes, attribution manifest and relevant earlier
  work (or `none` for an independent redo).
- The exact new report path and a requirement to read
  `orx skill deep-research/references/report-writer.md` first.
- Unresolved contradictions and absent notes; no new research, experiment
  launches, external publication or claims beyond supplied evidence.

The writer follows [references/report-writer.md](references/report-writer.md).
When its completion arrives, verify the file exists, read the final report,
and spot-check its key quantitative claims against cited notes. Unsupported
claims must be removed or qualified through a bounded writer revision, not
another research round. Record real harness/model contributions; agreement
between models is not validation of a factual claim.

Deliver a short 3–5 sentence answer with the main finding, material limitations,
and the report link. In OpenResearch use the session's artifacts link contract
with the full relative path, e.g.
`<file path="artifacts/topic/reports/Research title.md" />`.
Creating a report does not authorize publishing it or sending it to others.
