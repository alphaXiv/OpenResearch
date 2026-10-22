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
- On Linux, an active or successful local run establishes the mount namespace
  for that project's fixed command for this boot. A different namespace is
  rejected before creating a run; launch from the same mounted environment
  rather than repairing the command. Legacy runs without namespace metadata
  cannot establish this check. Namespace identity does not validate mounts
  within a namespace or discover missing dependencies on the first launch.
- A detached `orx supervise` process watches the run; do not kill it.
