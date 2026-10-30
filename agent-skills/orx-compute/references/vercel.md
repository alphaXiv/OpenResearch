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
  When `VERCEL_TOKEN` and `VERCEL_TEAM_ID` are set, orx reads the team plan
  once at launch and caps vCPUs at 4 on Hobby, 8 on Pro, and 32 on Enterprise.
- Timeout defaults to 4 hours, or 40 minutes on Hobby. A session lasts at most
  45 minutes on Hobby and 24 hours on Pro and Enterprise
  (https://vercel.com/docs/sandbox/pricing). orx keeps 5 minutes after the job
  to read the exit status and the last log lines, so `--timeout` can be at most
  40m on Hobby and 23h55m otherwise. A longer value is rejected before submit
  when the plan is known. Without a token and team id (OIDC only, or a failed
  team lookup), orx keeps the 4-hour default and the 23h55m cap and does not
  block; a plan limit then shows up as a sandbox create error.
- `--image` overrides the default `vercel/sandbox/python:3.14`, which includes
  `uv` and GNU `tail`. Images come from Vercel Container Registry. A custom
  image without GNU `tail --pid` (busybox or alpine) still streams logs: orx
  detects that and polls the log until the job exits. If the image cannot read
  the log at all, the supervisor reports an error instead of an empty log.
- `ORX_VERCEL_REGION` picks a region; otherwise Vercel uses the project default.
- `orx` copies the committed snapshot into the sandbox; Vercel never needs
  repository access.
- A detached `orx supervise` process records status and logs, then stops and
  destroys the sandbox when the run ends; do not kill it.
