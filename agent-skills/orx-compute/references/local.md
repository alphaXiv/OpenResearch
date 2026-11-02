# This machine (`--backend local`)

Use this backend only when the user asks to run on this machine or it is the
configured default. It starts a detached process using this machine's own
environment and shares its CPU, RAM, and GPU with other work.

```sh
orx exp run <expId> --backend local
```

- There is no flavor, host, image, or timeout flag.
- Prefer this backend for small or CPU-scale work; use remote compute for heavy
  jobs unless the user requests otherwise.
- The committed snapshot is extracted into an isolated run directory before
  executing the fixed command. Never train directly from the worktree.
- Runs live under `<orx data dir>/local-runs/<runId>/`. Cancellation terminates
  the process group.
- Finished runs older than each project's three newest local runs lose
  `repo/.venv` and `repo/.cache/uv`; logs, TensorBoard files, and other outputs stay.
- A detached `orx supervise` process watches the run; do not kill it.
