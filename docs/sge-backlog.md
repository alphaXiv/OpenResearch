# SGE backlog

The Sun Grid Engine backend covers the single-job case end to end: submit with
`qsub`, poll with `qstat`, cancel with `qdel`, and read the verdict from the
job's own `exit_code` file. The items below were deliberately left out of that
first cut. None of them block a normal run; each is written down so the reason
for the gap does not have to be rediscovered.

## Job shapes

| | |
|---|---|
| Array jobs | `#$ -t 1-N` is the natural fit for a parameter sweep, but one submission would then produce N results. That needs a run-per-task model in the store before the scheduler side is worth writing. |
| MPI parallel environments | Only `-pe omp <slots>` is emitted today, which is single-node by construction. Multi-node work needs `-pe mpi*` and a launcher contract with the run command. |
| Interactive sessions | `qrsh` and `qlogin` allocate a shell rather than a batch job. Neither fits the detached `orx supervise` lifecycle as it stands. |

## Resource selection

| | |
|---|---|
| `-l gpu_c`, `-l gpu_memory` | Compute capability and per-GPU memory are often better selectors than a model name — "at least 40 GB" beats enumerating which of A100, H200 and L40S qualify. Both are reachable through the `--flavor` `key=value` escape hatch, just not as first-class flags. |
| `mem_per_core`, `scratch_free` | Real settings rather than entries in `extraL`. The escape hatch works but is untyped, unvalidated, and invisible to the Settings UI. |

## Configuration

| | |
|---|---|
| Per-project `workDir` | `workDir` is one global setting. Labs that hold separate `/projectnb` allocations per research direction need an override per OpenResearch project. |
| `~/.ssh/config` stanza | orx multiplexes over its own private ControlPath, so a plain `ssh scc1` does not prime it and only `orx ssh connect` does. An opt-in stanza writing that `ControlPath` into the user's own config would make the two interchangeable — opt-in because it edits a file orx does not own. |

## Correctness

| | |
|---|---|
| Job-id recycling | SGE wraps job ids at ~9,999,999, far sooner than Slurm. A stale descriptor could therefore match a stranger's job. Reading `exit_code` before consulting the scheduler already covers the common case, since the file is per-run and unambiguous. Recording the submit time in the descriptor and cross-checking it against `qacct`'s `qsub_time` would close the gap entirely. |
