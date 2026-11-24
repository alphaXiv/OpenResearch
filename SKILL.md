---
name: openresearch-cli
description: Use the `orx` CLI to run local OpenResearch projects from a terminal — create experiment branches, launch and supervise compute, inspect logs and evidence, and manage the experiment tree. Read this before driving `orx` programmatically.
---

# OpenResearch CLI (`orx`)

`orx up` owns projects, experiment branches and runs in a local Git repository
and database. Read and edit a node's code with normal tools in its worktree
(`orx-git`).

The procedures live in modules, listed with when to load each at the end of
this output. Load the matching module before acting in its area with
`orx skill <name>`, and a reference it names with `orx skill <name>/<reference>`.
For a command no module covers, run it with `--help`.

To drive a project, start with `orx-experiment-tree`: its four rules govern
every experiment, and its loop turns each finished run into one of four moves:
**repair** a node whose run answered nothing, **refill** the round with another
sibling, **promote** the winner and descend, or **stop**. `orx-project-admin` finds
the project, experiment and run ids commands take.

Local commands need no sign-in. When a compute or account command reports
`Not logged in`, ask the user to run `orx login`.
