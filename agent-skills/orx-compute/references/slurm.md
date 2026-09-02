# Slurm (`--backend slurm`)

Use this backend only when the user explicitly requests their Slurm cluster or
it is the configured default. `orx` reaches the login node over SSH, stages the
committed snapshot, and submits the fixed command with `sbatch`.

```sh
orx exp run <expId> --backend slurm --host login-node --flavor h100:2 --timeout 4h
orx exp run <expId> --backend slurm
```

- `--host` is an alias from `~/.ssh/config`; omit it only when a Slurm default
  host is configured.
- `--flavor` is an optional GRES GPU request such as `h100:2`. Omit it for CPU.
- There is no image flag. The cluster environment—modules, conda, and login
  profile—is used as-is.
- Timeout defaults to 4 hours and covers the batch job.
- A detached `orx supervise` process records scheduler status and logs; do not
  kill it.
