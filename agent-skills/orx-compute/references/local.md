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

## Check this machine before every launch

A local run shares RAM with the user's desktop, the dashboard, and you. Read
the current headroom first:

```sh
orx compute show local --json
```

It reports `memBytes` (total), `memAvailableBytes` (free for a new workload
now), swap, `diskFreeBytes`, `loadAverage`, `cpuCount`, any NVIDIA GPUs with
free memory, and `launchFloorBytes`.

- Estimate the run's peak RAM: the dataset or batch it holds in memory, the
  model weights at their precision, optimizer state, and worker processes.
- Launch locally only when that estimate fits in `memAvailableBytes` with at
  least 25% to spare. Otherwise shrink the working set (smaller batch, fewer
  data-loader workers, streaming instead of loading everything, lower
  precision) or ask the user to switch to remote compute such as Colab.
- Leave cores for the machine: set thread and worker counts below `cpuCount`
  when `loadAverage` is already high.
- `orx exp run` refuses a local launch when free RAM is below
  `launchFloorBytes`, and orx stops a running local job when RAM and swap are
  nearly exhausted. Such a run fails with a reason that says so; treat it like
  an OOM and reduce memory use before relaunching.
- The committed snapshot is extracted into an isolated run directory before
  executing the fixed command. Never train directly from the worktree.
- Runs live under `<orx data dir>/local-runs/<runId>/`. Cancellation terminates
  the process group.
- A detached `orx supervise` process watches the run; do not kill it.
