---
name: orx-compute
description: "Launch and monitor experiment runs and route guidance for hf, modal, colab, k8s, ssh, slurm, ray, OpenResearch, Tinker, and local backends. Covers the run contract, GPU and RAM sizing, cancellation, and wait versus wake. Use before any launch or relaunch, when authoring a k8s manifest, choosing or switching compute, or handling an OOM, stall, or timeout; then read one backend reference."
---

Each run uses an immutable snapshot of the experiment branch's recorded commit.
Remote backends receive that snapshot; Tinker extracts it for a local controller
whose SDK sends model operations remotely.

```sh
orx exp status <expId>                 # branch, parent, run command, latest run + commit
orx compute prices --json              # credit left and GPU prices on every connected provider
orx compute                            # browse OpenResearch marketplace GPU offers
orx compute --gpu H100_SXM --count 1   # filter GPU offers
orx compute --cpu                      # browse CPU-only offers
orx exp run <expId>                    # launch on the configured default
orx exp cancel <expId>                 # cancel the in-flight run
```

## Inspect configuration and custom instructions first

Before configuring compute or constructing a launch command:

```sh
orx compute status --json
orx compute instructions show --json
```

`instructions show` returns the machine-wide `CUSTOM.md` content, absolute path,
and revision. Missing or blank content means there are no custom instructions.
Read the section for the selected backend/host, then its backend guide below.
For configuration, connection tests, login, or instruction updates, read
[references/configuration.md](references/configuration.md)
(`orx skill compute/configuration` is the fallback).

Usually leave `CUSTOM.md` empty. Add verified, consistently repeated bespoke
compute instructions only when standard CLI settings and committed project
scripts cannot express the workflow and the agent truly needs extra steps.
Keep any such guidance short: working commands, environment paths, cluster
constraints, and documentation links.
Use `orx compute instructions set --file - --expected-revision <revision from show>`;
never write the file directly with file-edit tools or shell redirects.
Preserve the user's guidance; replace obsolete steps instead of appending run
logs. Never store credentials or invent cluster commands. Read linked cluster
instructions before using unfamiliar scheduler options. Instructions do not
change the configured backend or authorize bypassing the launch contract.

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
<expId>` uses it. Use another backend only when the user names one or the
playbook says compute routing is on; a connected credential alone is not a
signal to switch.

## Route runs across providers

When the playbook says compute routing is on, pick the backend for each run
yourself, but only among the backends it lists. Run `orx compute status --json`
for which of them are connected and ready, and `orx compute prices --json` for
each provider's remaining credit and per-hour GPU prices.

- Fit first: the run's peak GPU memory, GPU count, RAM, and expected duration
  (each backend's guide says how to size it). Then cost: the cheapest ready
  option that fits, by `usdPerHour` times the expected hours. Then
  availability: skip an offer with `available: false`, and a provider that
  just failed with a capacity error, and use the next one.
- Never start a run that its provider's credit cannot finish. On a `credits`
  provider (Colab), an offer's `runwayHours` is how long the balance lasts at
  that rate; when the expected duration plus a 25% margin exceeds it, and the
  run does not checkpoint and resume, choose another provider or a cheaper
  flavor. Subtract runs already in flight on that provider
  (`balance.burningPerHour`). `usage` providers bill afterwards and do not run
  out mid-run; `own` providers (local, ssh, slurm, k8s, ray) cost nothing per
  hour but are limited by their hardware and quota.
- `estimated: true` marks a published or third-party price, not the account's
  own rate. Re-read with `--fresh` before a long or expensive launch.
- Smoke-test and debug on the cheapest option (`local`, Colab `cpu`/`t4`)
  before spending on large GPUs.
- Independent experiments may run at the same time on different providers to
  finish sooner. Never split one experiment's run across providers.
- Results compared head to head must come from the same hardware class, or the
  comparison must say which GPU each came from. Training speed and, at times,
  numerics differ between GPUs.
- Read the guide for each backend you route to before its first launch, and
  record the backend and flavor of every run in the experiment notes.

Before constructing the launch command, read exactly one reference relative to
this `SKILL.md`:

| Backend | Required guide |
|---|---|
| Hugging Face Jobs (`hf`) | [references/hf.md](references/hf.md) |
| Modal (`modal`) | [references/modal.md](references/modal.md) |
| Google Colab (`colab`) | [references/colab.md](references/colab.md) |
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
  retryable; failures after startup require reading the file located by `orx logs <runId>`.
- A failed run is not a new node. Repair and relaunch the same experiment as
  described by `orx-experiment-tree`.

### Going idle instead — `orx exp wake`

After launching, use `orx exp wake <expId>` when you want to end the turn and
resume after that run succeeds or fails. Wake-up is opt-in, fires only for
`done` or `failed`, and waits behind queued user messages. If orx cannot
monitor a live Slurm run for over a minute, it tells you once per outage; the
final wake-up comes once orx can see the run finish. Use either wait or wake
for a run, not both.

## Sizing compute

- Decide GPU versus CPU first. API-driven evaluation and data preparation often
  run more cheaply on CPU.
- Estimate peak memory before choosing: accelerator memory for GPU runs, host
  RAM for CPU runs. The Colab guide has the rules of thumb for model weights,
  optimizer state, and activations.
- Pick the smallest shape that fits the model and a minimal batch.
- Before a run on this machine, read its free RAM with `orx compute show local
  --json` and follow the checks in the local guide.
- Escalate after a real OOM or hopelessly slow run instead of starting with the
  largest accelerator.
- Raise the timeout only for genuinely long runs.
