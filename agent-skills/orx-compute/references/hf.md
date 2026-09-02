# Hugging Face Jobs (`--backend hf`)

Use this backend only when the user explicitly requests Hugging Face Jobs or it
is the configured default. A connected token only makes the backend available;
it does not select it.

Jobs run in the user's Hugging Face account and are billed there. Authentication
uses `HF_TOKEN` from the environment.

```sh
orx exp run <expId> --backend hf --flavor a10g-small
orx exp run <expId> --backend hf --flavor a100-large --timeout 8h
orx exp run <expId> --backend hf --flavor cpu-upgrade --image python:3.12
```

- `--flavor` is required unless the configured default includes one. Common GPU
  flavors include `t4-small`, `a10g-small`, `a10g-large`, `l4x1`, `l40sx1`,
  `a100-large`, `h100`, and `h200`, with `x2`/`x4`/`x8` variants. CPU options
  include `cpu-basic` and `cpu-upgrade`.
- Timeout defaults to 4 hours and covers the whole job. A timeout is recorded as
  a failed run.
- `--image` overrides the container. Defaults are a CUDA PyTorch image for GPU
  shapes and `python:3.12` for CPU shapes.
- `orx` uploads the committed snapshot into a private job volume. The job never
  clones a repository or needs repository credentials.
- A detached `orx supervise` process records provider status and logs; do not
  kill it.
