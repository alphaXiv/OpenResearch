# OpenResearch (`--backend openresearch`)

Use this backend only when the user explicitly requests an OpenResearch box or
it is the configured default. It provisions an ephemeral machine billed to the
user's organization and deletes it when the run ends. It requires `orx login`
and a registered SSH key.

```sh
orx exp run <expId> --backend openresearch --flavor h100_sxm:2 --timeout 4h
orx exp run <expId> --backend openresearch --flavor cpu5c:32 --org <orgId>
```

- `--flavor` is a GPU id from `orx compute`, optionally with a count, or a CPU
  family such as `cpu5c`, `cpu5g`, or `cpu5m` with optional `:vcpus`.
- Optional flags include `--org`, `--disk`, and `--provider`.
- There is no image flag; the platform image is fixed.
- Timeout defaults to 4 hours. The machine and its storage disappear when the
  run ends, so all evidence must reach the run log.
- `orx` streams the committed snapshot to the provisioned machine.
- A detached `orx supervise` process records machine status and logs; do not
  kill it.
