---
name: orx-python-env
description: "Choose and set up Python environments. Use before installing packages, creating or syncing an environment, or running a Python utility that needs a third-party package."
---

## Choose the tooling

- Follow the user's instructions and the project's established dependency
  tooling; inspect its setup first. `pyproject.toml` alone does not imply uv.
  Otherwise prefer uv when it is available on the execution host.
- Without uv, use the existing Python or `venv` and pip. Never install uv, and
  never stop only because it is missing. Preserve metadata, report
  incompatibilities, and don't treat pip as reproducing `uv.lock`.
- Initialize a blank Python project with uv when needed and available: derive a
  valid package name from the project, and declare and lock its dependencies.

## One-off utilities

Check for uv first and run
`uv run --isolated --no-project --with <package> python ...`. For PDF
extraction the package and the import are both `pymupdf`. Don't assume bare
`python` exists or that system Python has the package; without uv, use a
verified environment or a venv with it installed. Keep one-off utilities out of
the project's dependency files.

## Project environments

- For uv projects, use `uv run --locked`, preserve configuration, and commit
  dependency declarations and locks together. Fix a stale lock rather than
  bypassing it.
- Select the environment explicitly. Keep ignored environments separate per
  worktree; never copy or share `.venv`. Reuse uv's default cache. Share
  dependency changes through Git, reconcile environments after a checkout or an
  integration, and coordinate edits in a shared worktree.
- Establish the baseline's dependencies before branching experiments. A run
  recipe recreates its dependencies from the committed snapshot, independent of
  any session environment; preserve the fixed run contract.
