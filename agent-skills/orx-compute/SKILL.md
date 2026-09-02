---
name: orx-compute
description: "Launch and monitor experiment runs and route guidance for hf, modal, k8s/Kubernetes, ssh, slurm, ray, OpenResearch, Tinker, and local backends. Covers the fixed run contract, sizing, cancellation, and wait versus wake. Use before any launch or relaunch, when authoring a k8s manifest, choosing or switching compute, or handling an OOM, stall, or timeout; then read one backend reference."
---

Each run uses an immutable snapshot of the experiment branch's recorded commit.
Remote backends receive that snapshot; Tinker extracts it for a local controller
whose SDK sends model operations remotely.

```sh
orx exp status <expId>                 # branch, parent, run command, latest run + commit
orx compute                            # browse GPU offers across all providers
orx compute --gpu H100_SXM --count 1   # filter GPU offers
orx compute --cpu                      # browse CPU-only offers
orx exp run <expId>                    # launch on the configured default
orx exp cancel <expId>                 # cancel the in-flight run
```

## Universal launch contract

- **Launch all experiment compute with `orx exp run`.** Never invoke provider
  CLIs, schedulers, raw SSH, or the training command directly. The worktree is
  for editing, Git, orchestration, and lightweight checks; direct jobs are
  untracked and may run code other than the recorded commit.
- **Keep the run command fixed.** Set it once on the baseline and vary code or
  configuration on child branches. If none exists, use `orx project edit
  <projectId> --run-command '<cmd>'` before launching.
- **Commit before launching.** Every backend runs the recorded commit's
  immutable source snapshot. Uncommitted files are excluded; no backend needs a
  GitHub push.
- `orx exp run` queues the run and returns immediately. Follow it with `orx
  runs`, `orx logs`, `orx exp wait`, or `orx exp wake`.
- `--force` permits a deliberate concurrent run on the same experiment.
  Without it, `orx` rejects a launch while that node already has a run in
  flight.

## Resolve the backend, then read one guide

The session playbook states the configured default. A bare `orx exp run
<expId>` uses it. Use another backend only when the user names one; a connected
credential is not a signal to switch.

Before constructing the launch command, read exactly one reference relative to
this `SKILL.md`:

| Backend | Required guide |
|---|---|
| Hugging Face Jobs (`hf`) | [references/hf.md](references/hf.md) |
| Modal (`modal`) | [references/modal.md](references/modal.md) |
| Kubernetes (`k8s`) | [references/k8s.md](references/k8s.md) |
| SSH (`ssh`) | [references/ssh.md](references/ssh.md) |
| Slurm (`slurm`) | [references/slurm.md](references/slurm.md) |
| Ray Jobs (`ray`) | [references/ray.md](references/ray.md) |
| OpenResearch (`openresearch`) | [references/openresearch.md](references/openresearch.md) |
| Tinker (`tinker`) | [references/tinker.md](references/tinker.md) |
| This machine (`local`) | [references/local.md](references/local.md) |

Do not read guides for backends you are not using. Kubernetes manifest work
always requires `references/k8s.md` before creating or editing the manifest.
If the installed reference cannot be read, `orx skill compute/<backend>` prints
the same canonical document.

## Waiting on runs — `orx exp wait`

Block until a run changes state when you want to act as soon as it finishes:

```sh
orx exp wait <expId>                    # wait for this experiment's latest run
orx exp wait --project <projectId>      # return on the first project completion
orx exp wait <expId> --interval 10 --timeout 3600
```

- Pass exactly one of `<expId>` or `--project`.
- `--project` is the budget-loop primitive: it returns on the first completion,
  not on starts or queued-to-running transitions. Reissue it once per loop tick.
- A wait is a sleep-until-change signal, not the source of truth. After every
  return, read `orx runs <projectId>` and reconcile all newly terminal runs.
- When nothing is in flight, project wait returns `drained: no runs in flight`.
- The default interval is 5 seconds and timeout is 1800 seconds. Timeout exits
  non-zero and means nothing changed yet, not that the run failed.
- Failed runs include a `reason:` line. Provider-capacity failures are often
  retryable; failures after startup require reading `orx logs <runId>`.
- A failed run is not a new node. Repair and relaunch the same experiment as
  described by `orx-experiment-tree`.

### Going idle instead — `orx exp wake`

After launching, use `orx exp wake <expId>` when you want to end the turn and
resume after that run succeeds or fails. Wake-up is opt-in, fires only for
`done` or `failed`, and waits behind queued user messages. Use either wait or
wake for a run, not both.

## Sizing compute

- Decide GPU versus CPU first. API-driven evaluation and data preparation often
  run more cheaply on CPU.
- Pick the smallest shape that fits the model and a minimal batch.
- Escalate after a real OOM or hopelessly slow run instead of starting with the
  largest accelerator.
- Raise the timeout only for genuinely long runs.
