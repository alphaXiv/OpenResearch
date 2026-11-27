# boxd (`--backend boxd`)

Use this backend only when the user explicitly requests boxd or it is the
configured default. Each run gets its own isolated
[boxd](https://boxd.sh?ref=OpenResearch) machine: a full Ubuntu 24.04 VM with
Python, `uv`, Node and Docker. It is CPU-only: choose another backend for GPU
work.

Authentication uses `BOXD_API_KEY`, which orx saves when the user connects boxd
(`orx compute connect boxd` prompts for it). Machines go to the key's own org,
and the key never reaches the job.

```sh
orx exp run <expId> --backend boxd --flavor 2 --timeout 30m
```

- `--flavor` is optional: `cpu` (the org's default size), `cpu-<n>` or `<n>`,
  where n is 1, 2, 4 or 8, with 4 GiB of memory per vCPU. A size above the
  org's limit is rejected before anything is created.
- `--timeout` defaults to 4 hours. The job is stopped when it runs past it.
- `--image` is refused. Install what the run needs in its command, for
  example `uv run --with numpy python train.py`.
- `ORX_BOXD_REGION` picks `fra` or `nyc`, else the org's default region.
  `ORX_BOXD_NAME_PREFIX` sets the machine name prefix (default `orx`).
- The machine is billed while it runs, at boxd's prices
  (https://boxd.sh/pricing?ref=OpenResearch). orx shows the most it can cost
  per hour.

OpenResearch deletes the machine once the run ends and its log is collected.
If OpenResearch is closed when the job ends, the machine keeps running until
OpenResearch comes back and deletes it.

See [run monitoring](watching.md).
