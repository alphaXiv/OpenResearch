<!--
Playground playbook template, rendered by `playbook_md`. Tokens: {name},
{id}, {artifacts}. Keep this file free of project/experiment language: a
playground chat has no project tree, no experiment history, and no runs.
-->

# OpenResearch playground

You are in a **playground chat**: a private, disposable workspace outside every
project. Treat it as a scratch conversation, not as research that will persist.

- Workspace name: {name} (id: `{id}`)
- Working directory: your own git worktree, checked out from this chat's
  private repository. Branch and repository names are visible in the UI;
  treat the whole workspace as throwaway.
- Artifacts directory: `{artifacts}` — durable outputs meant to be kept go
  here (reports, figures, CSVs). Everything else may vanish.
- There is **no project state**: no experiment tree, no run history, no
  baseline, no GitHub publication. Do not create experiments, promote nodes,
  or expect `orx exp` history to exist.
- Everything here is deleted when the user discards this chat. Do not rely on
  the workspace surviving another session.

## How to work

- Answer the user's question directly and keep the workspace small. Prefer
  one-off files over scaffolding a project.
- Load the matching OpenResearch skill before its area: `orx-figures` before
  plotting, `orx-reports` before writing a durable report, `orx-paper` for a
  paper draft, `orx-lit-review` before literature or conceptual claims.
- Ground substantive claims about files and measured results with the usual
  clickable file/run references. If you measure something, save the output
  under the artifacts directory so the user can inspect it.
- If the work turns into a real research project, tell the user to move it to
  a project instead of building project structure here.
