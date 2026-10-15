# Kaggle (`--backend kaggle`)

Use this backend only when the user explicitly requests Kaggle or it is the
configured default (or the playbook's compute routing lists it). Each run is a
private script kernel on one of the user's Kaggle accounts and spends that
account's free weekly GPU or TPU quota; it costs no money.

`orx` drives Kaggle through the official Kaggle CLI (`kaggle`) on this machine.
It needs Linux or macOS, the CLI installed (`uv tool install kaggle`), and at
least one saved account. The user can save several accounts; each keeps its
own credentials and weekly quota. Check readiness first:

```sh
orx compute test kaggle
orx compute show kaggle --json      # CLI path and version, accounts, and each account's quota
```

`accounts` in that output lists every saved account with `active` (used when a
launch names none) and `quota.gpu` / `quota.tpu` (`usedHours`,
`remainingHours`, `totalHours`; `refreshAt` is when the week resets). `quota`
is `null` when Kaggle did not say; `error` explains an account that failed.

```sh
orx exp run <expId> --backend kaggle                    # two T4s on the active account
orx exp run <expId> --backend kaggle --flavor p100
orx exp run <expId> --backend kaggle --flavor t4@auto   # the account with the most GPU hours left
orx exp run <expId> --backend kaggle --flavor tpu@alice --timeout 6h
```

## Choose the accelerator and account

| Flavor | Accelerator | Memory | Typical fit |
|---|---|---|---|
| `cpu` | CPU only (4 vCPU, ~30 GB RAM) | none | data prep, API-driven evals; uses no quota |
| `t4` | 2× NVIDIA T4 | 15 GB each | the default; inference up to ~7B in 4-bit, small fine-tunes, DDP across both GPUs |
| `p100` | NVIDIA P100 | 16 GB | single-GPU fp32 code; no bf16 and no fast fp16 tensor cores |
| `tpu` | TPU VM v3-8 | 16 GB per core | JAX or PyTorch/XLA code written for TPUs only |

- Size the run the way the Colab guide does (parameters × bytes per parameter,
  plus activations); a single T4 holds 15 GB, so a model that needs more must
  shard across both T4s or use another backend.
- GPU runs share one weekly GPU quota per account (usually 30 hours); TPU runs
  have their own. Estimate the run's hours before launching and compare them
  with `remainingHours`. Use `@auto` to pick the account with the most hours
  left, or `@<account>` to keep a series of runs on one account.
- A session ends after 12 hours (9 on TPUs) whatever `--timeout` says; orx
  refuses longer timeouts. Split longer work into resumable runs.

## Behavior

- Timeout defaults to 4 hours and bounds the command inside the kernel.
- The committed snapshot is uploaded as a private dataset, mounted in the
  kernel, and extracted into `/tmp/orx-run/repo`; the dataset is deleted when
  the run ends. Keep datasets and checkpoints out of Git and download them
  inside the run (the kernel has internet access).
- Synced secrets such as `HF_TOKEN` are **not** sent: the kernel's code is
  stored on Kaggle, so orx never writes secrets into it. A run that needs a
  secret belongs on another backend.
- Files the run writes to `$ORX_OUTPUT_DIR` (`/kaggle/working`) are downloaded
  into the run's `kaggle-output` folder when it ends, with the full kernel log.
- GPUs and internet access need a phone-verified Kaggle account; Kaggle
  refuses the kernel otherwise, and the run fails with that reason.
- The kernel may wait in Kaggle's queue before it starts; the log says so.
- A detached `orx supervise` process records status and logs; do not kill it.
  Cancelling the run stops following the kernel, but Kaggle keeps it running
  until it ends or reaches its timeout; the log links the kernel page where the
  user can stop it sooner.
