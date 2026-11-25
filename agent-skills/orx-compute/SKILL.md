---
name: orx-compute
description: "Launch, size, price and stop runs on any backend. Use before any launch or relaunch, when choosing or configuring compute, when a run OOMs, stalls or times out, or for a standalone machine."
---

Each run uses an immutable snapshot of the experiment branch's recorded commit.
Remote backends receive that snapshot; Tinker extracts it for a local
controller whose SDK sends model operations remotely.

```sh
orx exp status <expId>                 # branch, parent, run command, latest run: stage, estimate, measures
orx exp run <expId>                    # launch on the configured default
orx exp cancel <expId> --reason '…'    # cancel the in-flight run, saying why
orx metrics <runId> [<runId>…]         # what runs logged to Trackio
```

## Launch contract

- **Launch all experiment compute with `orx exp run`.** Never invoke provider
  CLIs, schedulers, raw SSH, or the training command directly: direct jobs are
  untracked and may run code other than the recorded commit. The worktree is
  for editing, Git, orchestration, and lightweight checks.
- **Keep the run command fixed.** Set it once on the baseline (if none exists,
  `orx project edit --run-command '<cmd>'`) and vary code or
  configuration on child branches.
- **Commit before launching.** Uncommitted files are excluded; no backend needs
  a GitHub push.
- **Estimate before every launch,** unasked: run
  `orx exp run <expId> --estimate` with the `--backend` and `--flavor` the
  launch will use, and give the user its time, cost and hardware in one line
  before launching, or in the plan when planning; name the hardware that timed
  it when that differs from the launch's. Run it the same way when the user
  asks what a run would cost on some compute.
- `orx exp run` queues the run and returns immediately; follow the post-launch
  check in `orx-experiment-runs`. Without `--force`, `orx` rejects a launch
  while the experiment already has a run in flight; pass it only for a
  deliberate concurrent run.
- The launching chat is subscribed automatically: results arrive once it is
  idle, possibly several in one message. Silence does not mean a run failed.
  Never keep a turn waiting, with `orx exp wait` or with `sleep` over
  `orx runs`: it holds back the user's next message.
- A baseline reports its stages and measures (children inherit them) as
  `orx-experiment-runs` describes, using `assets/orx_run.py`. Signals are
  advisory: confirm trouble in the raw log before changing the experiment.

Read when:
- references/tracking.md — before logging metrics or reading `ORX_*` variables in a run, and for `orx metrics` or `orx tensorboard`
- references/sizing.md — before choosing GPU or CPU, a shape, or a timeout, or browsing offers
- references/result-messages.md — when a result, attention or cancellation message arrives
- references/watching.md — for a run launched elsewhere, an interim result, or a process watching a run

## Choose the backend, then read one guide

Before configuring compute or constructing a launch command:

```sh
orx compute status --json
orx compute instructions show --json   # machine-wide CUSTOM.md: content, path, revision
```

Missing or blank content means no custom instructions; otherwise read the
section for the selected backend or host. The session playbook states this
project's default, which a bare `orx exp run <expId>` uses. Use another
backend only when the user names one; a connected credential is not a signal
to switch. `orx compute default set` changes this project's default; add
`--machine` only when the user asks for every project. Read exactly one guide, never those for backends you are not using.
If an installed reference cannot be read, `orx skill compute/<name>` prints
`references/<name>.md`.

- references/hf.md — Hugging Face Jobs (`hf`)
- references/modal.md — Modal (`modal`)
- references/k8s.md — Kubernetes (`k8s`), always before creating or editing a manifest
- references/ssh.md — SSH (`ssh`)
- references/slurm.md — Slurm (`slurm`), including `--cpus`, `--mem` and `--flavor cpu`
- references/ray.md — Ray Jobs (`ray`)
- references/vercel.md — Vercel Sandbox (`vercel`)
- references/openresearch.md — OpenResearch (`openresearch`)
- references/tinker.md — Tinker (`tinker`)
- references/local.md — this machine (`local`)

Read when:
- references/configuration.md — to configure a backend, test a connection, log in, save credentials, edit SSH aliases, or change custom instructions
- references/instances.md — when the user wants a persistent machine for manual work rather than a run
