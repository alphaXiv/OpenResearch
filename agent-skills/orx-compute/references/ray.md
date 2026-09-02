# Ray Jobs (`--backend ray`)

Use this backend only when the user explicitly requests a Ray cluster or it is
the configured default. It submits through the Ray Jobs/Dashboard API and uses
the cluster's runtime environment.

```sh
orx exp run <expId> --backend ray
orx exp run <expId> --backend ray --flavor gpu:1
orx exp run <expId> --backend ray --flavor cpu:2,mem:8GiB
```

- The address comes from saved Ray configuration, then
  `ASTROAI_RAY_JOBS_ADDRESS` or `RAY_DASHBOARD_URL`, then
  `http://127.0.0.1:8265`.
- Optional `--flavor` hints use `cpu[:N]`, `gpu[:N]`, and `mem:<size>`, joined
  with commas. Memory is a scheduling reservation, not an enforced cap.
- Omit the flavor to reserve nothing, which avoids Pending on small heads.
- There is no image, host, or timeout flag. Bound the work in the run command.
- Ray receives the committed snapshot as its `working_dir` package.
- A detached `orx supervise` process records job status and logs; do not kill
  it.
