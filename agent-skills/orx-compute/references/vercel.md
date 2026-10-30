# Vercel Sandbox (`--backend vercel`)

Use this backend only when the user explicitly requests Vercel Sandbox or it is
the configured default. It runs an ephemeral Firecracker microVM on the user's
Vercel project, billed for active CPU and provisioned memory. It is CPU-only:
choose another backend for GPU work.

Authentication uses `VERCEL_TOKEN`, `VERCEL_TEAM_ID` and `VERCEL_PROJECT_ID`,
or a `VERCEL_OIDC_TOKEN` from `vercel env pull` (it expires after 12 hours).
`orx` provisions its managed Vercel SDK environment on the first launch and
never passes these credentials into the job.

```sh
orx exp run <expId> --backend vercel
orx exp run <expId> --backend vercel --flavor cpu-8 --timeout 8h
orx exp run <expId> --backend vercel --flavor 4 --image vercel/sandbox/universal
```

- `--flavor` is optional: `cpu` (2 vCPUs, the default), `cpu-<n>` or `<n>`,
  where n is 1 or an even number up to 32. Each vCPU comes with 2 GB of memory.
  Plans cap vCPUs (Hobby 4, Pro 8, Enterprise 32).
- Timeout defaults to 4 hours. A session lasts at most 24 hours (45 minutes on
  Hobby).
- `--image` overrides the default `vercel/sandbox/python:3.14`, which includes
  `uv`. Images come from Vercel Container Registry.
- `ORX_VERCEL_REGION` picks a region; otherwise Vercel uses the project default.
- `orx` copies the committed snapshot into the sandbox; Vercel never needs
  repository access.
- A detached `orx supervise` process records status and logs, then stops and
  destroys the sandbox when the run ends; do not kill it.
