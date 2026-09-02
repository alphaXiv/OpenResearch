# Modal (`--backend modal`)

Use this backend only when the user explicitly requests Modal or it is the
configured default. Modal runs an ephemeral Sandbox in the user's account and
bills that account per second.

Authentication uses `MODAL_TOKEN_ID` and `MODAL_TOKEN_SECRET`, or credentials
created by `modal token new`. `orx` provisions its managed Modal environment on
the first launch.

```sh
orx exp run <expId> --backend modal --flavor a10g
orx exp run <expId> --backend modal --flavor a100-80gb --timeout 8h
orx exp run <expId> --backend modal --flavor h100:2
orx exp run <expId> --backend modal --flavor cpu --image python:3.12
```

- `--flavor` is required unless the configured default includes one. GPU values
  include `t4`, `l4`, `a10g`, `a100`, `a100-80gb`, `l40s`, `h100`, and `h200`;
  append `:N` for multiple GPUs. CPU values are `cpu` and `cpu-large`.
- Timeout defaults to 4 hours and covers the whole Sandbox.
- `--image` overrides the default CUDA PyTorch or CPU Python image.
- `orx` copies the committed snapshot into the Sandbox; Modal never needs
  repository access.
- A detached `orx supervise` process records provider status and logs; do not
  kill it.
