# Slurm (`--backend slurm`)

Use this backend only when the user explicitly requests their Slurm cluster or
it is the configured default. `orx` reaches the login node over SSH, stages the
committed snapshot, and submits the fixed command with `sbatch`. The login node
needs Python 3.6 or newer for supervised runs; engine runs use `orx-host`.
No Python packages or administrator installation are required.
All runs on the same connection share one monitoring channel and batched scheduler
queries. Transport failures back off together, and fresh authentication requires
explicit Connect. Reconnect with `orx compute connect slurm --host login-node`.

```sh
orx exp run <expId> --backend slurm --host login-node --flavor h100:2 --timeout 4h
orx exp run <expId> --backend slurm --flavor h100:1 --cpus 16 --mem 128G
orx exp run <expId> --backend slurm
```

- `--host` is an alias from `~/.ssh/config`; omit it only when a Slurm default
  host is configured.
- `--partition` overrides the saved Slurm partition for one run. A list such as
  `gpu,batch` lets Slurm choose between partitions; omit it to use the saved or
  cluster default.
- `--flavor` is an optional GRES GPU request such as `h100:2`. Omit it for CPU.
- `--cpus` and `--mem` set `#SBATCH --cpus-per-task` and `#SBATCH --mem`
  (`64G`, `4000M`, or a bare number for megabytes). Unset falls back to
  the Slurm defaults saved by `orx compute configure slurm`, then the partition
  default, which is often one core and `DefMemPerCPU` of memory: too little to
  load most models. Size them for the workload, especially on GPU runs.
- There is no image flag. The cluster environment—modules, conda, and login
  profile—is used as-is.
- `--timeout` covers the batch job. Unset falls back to the Slurm time-limit
  setting, then the cluster's limit.
- With the new engine enabled, runs and cached source archives live under
  `~/.orx/engine` on the cluster unless `orx compute configure slurm --remote-root /scratch/alice/orx` names
  another absolute directory (runs then live in its `engine/` subdirectory),
  such as scratch space when home quota is small. It must be on a filesystem
  the compute nodes share. Each run keeps the root it was submitted with;
  `--clear remote-root` restores the default for new runs.
- Run dirs stay on the cluster after the run; OpenResearch keeps each run's
  log on this computer. Remove old dirs under `<root>/engine/runs/` when space
  runs short.
- Supervised runs (the stable/dev default) use `<root>/runs/`. Each project's three newest
  SSH and Slurm runs stay whole, older finished ones lose their remote run dir
  except `tensorboard/`, and idle cached source archives are removed.
  `orx compute configure slurm --keep-remote-runs true` keeps them all; deleting
  a project removes them regardless.

See [run monitoring](../SKILL.md#run-engine-setting).
