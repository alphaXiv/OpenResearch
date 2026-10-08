# Google Colab (`--backend colab`)

Use this backend only when the user explicitly requests Colab or it is the
configured default. Each run gets its own Colab runtime in the user's Google
account, billed against their Colab plan or compute units, and released when the
run ends or is cancelled.

`orx` drives the runtime through the official Colab CLI (`colab`, from
`google-colab-cli`) on this machine. It needs Linux or macOS, the CLI installed
(`uv tool install google-colab-cli`), and a one-time browser sign-in
(`orx compute connect colab`). Check readiness first:

```sh
orx compute test colab
orx compute show colab --json      # CLI path, sign-in, and every accelerator with its memory
```

```sh
orx exp run <expId> --backend colab                     # T4 (the default)
orx exp run <expId> --backend colab --flavor l4
orx exp run <expId> --backend colab --flavor auto:30    # smallest GPU with >= 30 GB VRAM
orx exp run <expId> --backend colab --flavor a100:highmem --timeout 8h
```

## Choose the accelerator for the task

You pick the flavor from the workload; do not default to the largest GPU.

| Flavor | Accelerator | Memory | Typical fit |
|---|---|---|---|
| `cpu` | CPU only | none | data prep, API-driven evals, small sklearn |
| `t4` | NVIDIA T4 | 16 GB | inference up to ~7B in 4-bit, fine-tuning small models (<1B), most classic CV/NLP |
| `l4` | NVIDIA L4 | 24 GB | 7B inference in bf16, LoRA on 7B in 4-bit, faster T4 workloads (bf16 support) |
| `a100` | NVIDIA A100 | 40 GB | LoRA on 7B to 13B, full fine-tuning up to ~3B, larger batches |
| `h100` | NVIDIA H100 | 80 GB | full fine-tuning up to ~7B, 30B+ inference, throughput-bound training |
| `g4` | NVIDIA RTX PRO 6000 class | 96 GB | the largest single-GPU memory on Colab |
| `v5e1`, `v6e1` | one TPU chip | 16 / 32 GB HBM | JAX or PyTorch/XLA code written for TPUs only |

Estimate the peak accelerator memory before choosing, then pass the smallest
flavor that fits with about 20% headroom, or `auto:<GB>` with that estimate:

- Inference: parameters × bytes per parameter (2 for bf16/fp16, 1 for int8,
  0.5 for 4-bit), plus the KV cache and activations.
- Full fine-tuning with Adam in mixed precision: about 16 bytes per parameter
  plus activations, which grow with batch size × sequence length.
- LoRA or QLoRA: the frozen base weights at their precision plus a small
  fraction for adapters, optimizer state, and activations.
- Classic models (ResNets, BERT-base, small transformers): T4 is usually enough.

Append `:highmem` when host RAM, not GPU memory, is the limit (large datasets
held in memory, data loading workers). L4 and TPUs have only one machine shape.
Escalate one step after a real CUDA out-of-memory error; the run log starts with
the GPU name and memory that Colab actually assigned.

## Behavior

- Timeout defaults to 4 hours and bounds the command on the runtime.
- The committed snapshot is uploaded to the runtime and extracted into
  `/content/orx-run/repo`; the fixed run command runs there with your synced
  environment variables (for example `HF_TOKEN`). Values are never written to disk.
- The whole snapshot is uploaded in one request, so keep datasets and
  checkpoints out of Git and download them inside the run.
- Everything on the runtime is deleted when it is released. Print metrics to
  stdout, and push checkpoints or large outputs to durable storage (for example
  the Hugging Face Hub) inside the run command.
- Availability depends on the user's Colab plan. If Colab cannot assign the
  accelerator, the run fails at once with that reason; pick another flavor or
  retry later.
- A detached `orx supervise` process records status and logs; do not kill it.
  Cancelling the run releases the runtime.
