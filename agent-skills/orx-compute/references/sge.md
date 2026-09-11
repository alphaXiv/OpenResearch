# Sun Grid Engine (`--backend sge`)

Use this backend only when the user explicitly requests their Grid Engine
cluster — BU's Shared Computing Cluster (SCC) — or it is the configured
default. `orx` reaches the login node over SSH, stages the committed snapshot,
and submits the fixed command with `qsub`.

```sh
orx exp run <expId> --backend sge
orx exp run <expId> --backend sge --host scc1 --flavor A100:2 --timeout 8h
orx exp run <expId> --backend sge --flavor cpu
```

- `--host` is an alias from `~/.ssh/config`; omit it when an SGE default host
  is configured.
- There is no image flag. The cluster environment — modules, conda, and the
  login profile — is used as-is. Jobs run under a login shell.
- A detached `orx supervise` process records scheduler status and logs; do not
  kill it.

## `--flavor`

Unlike Slurm, omitting `--flavor` still requests a GPU.

| Form | Meaning |
|---|---|
| omitted | the settings default (1 GPU) |
| `cpu` | no GPU |
| `2` | 2 GPUs of the default type |
| `A100:2` | 2×A100 |
| `L40S` | 1×L40S |
| contains `=` | passed through verbatim as `-l` requests |

The last form is the escape hatch for anything the table cannot express, e.g.
`--flavor 'gpus=1,mem_per_core=8G'`.

Valid `gpu_type` values on SCC: `A100`, `A40`, `A6000`, `H200`, `K2200`,
`K40m`, `L40`, `L40S`, `M2000`, `P100`, `RTX6000`, `RTX6000ada`, `RTX8000`,
`RTXP6000`, `TitanV`, `TitanXp`, `V100`. **There is no H100 — it is H200.**
Not every type is available to every project; `qsub -w v` verifies a job
without submitting it and is the way to check.

## Defaults and work directory

Defaults are 1×L40S, 16 cores (`-pe omp 16`), 12h (`-l h_rt=12:00:00`), and
project `herbdl`. They live in `$XDG_CONFIG_HOME/openresearch/sge.json`; the
user can also change them from their Sun Grid Engine compute settings.

Run dirs and the source cache live under an absolute `workDir` (default
`/projectnb/herbdl/workspaces/herb`, plus a per-user leaf), **not `$HOME`** —
SCC home directories are quota'd at 10 GB. The layout is
`<workDir>/<user>/.orx/runs/<runId>/` holding `repo/`, `job.qsub`, `log`, and
`exit_code`.

## The second factor is a prerequisite

SCC's sshd requires `publickey,keyboard-interactive`, and ssh `BatchMode=yes`
— which `orx` uses for every background call — cannot answer the second
factor. `orx` instead rides one multiplexed SSH session the user opens.

If a run stalls with an authentication message, the fix is:

```sh
orx ssh connect <host>
```

A plain `ssh <host>` will **not** help: `orx` uses its own private
ControlPath. A 30-day Duo grace per (source IP, login node) means this is
usually needed at most once a month, and a master lost to laptop sleep or a
network change is re-established automatically.

## Status

- The `exit_code` file is the primary truth; it is scheduler-independent.
- `qstat` gives live state.
- `qacct` is an ~11-second escalation, used only when a job leaves the queue
  without an exit code.
- An `Eqw` job is parked and will never run. `orx` reports the `qstat -j`
  error reason — usually a bad project, an unavailable `gpu_type`, or an
  unreachable working directory.

## Not supported yet

Array jobs, MPI parallel environments, and interactive `qrsh` sessions.
