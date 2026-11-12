---
name: orx-compute
description: "Launch and monitor experiment runs and route guidance for hf, modal, k8s/Kubernetes, ssh, slurm, ray, vercel, OpenResearch, Tinker, and local backends. Covers the fixed run contract, sizing, cancellation, and result messages. Use before any launch or relaunch, when authoring a k8s manifest, choosing or switching compute, or handling an OOM, stall, or timeout; then read one backend reference."
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
- `orx exp run` queues the run and returns immediately. End your turn once the
  ready runs are launched; relevant results arrive as messages in this chat.
- `--force` permits a deliberate concurrent run on the same experiment.
  Without it, `orx` rejects a launch while that node already has a run in
  flight.
- Every run gets `ORX_RUN_ID`, `ORX_EXPERIMENT`, `ORX_EXPERIMENT_TITLE`,
  `ORX_PARENT_EXPERIMENT` (when it has a parent), `ORX_COMMIT` and
  `TRACKIO_PROJECT`. Saved environment variables reach every backend except
  `hf`, which gets only the `ORX_*` and Trackio ones. To track metrics, log
  with Trackio; orx shows its charts beside the run:
  `trackio.init(project=os.environ.get("TRACKIO_PROJECT", <name>),
  name=os.environ.get("ORX_RUN_ID"), group=os.environ.get("ORX_EXPERIMENT"),
  config={"hypothesis": os.environ.get("ORX_EXPERIMENT_TITLE"), "parent":
  os.environ.get("ORX_PARENT_EXPERIMENT"), "commit": os.environ.get("ORX_COMMIT"),
  **hyperparameters})`. Keep that project and name, or orx cannot find the
  run. The client reads `TRACKIO_SPACE_ID` or `TRACKIO_SERVER_URL` and its
  token itself. With neither saved, only `local` runs show up in orx, and a
  localhost URL only works on the `local` backend. Never print a token.
- `orx metrics <runId> [<runId>…]` reads back what runs logged to Trackio,
  wherever it went: per metric its first, last, min and max (with steps), the
  mean over the last tenth, a sparkline, and sampled `step:value` points.
  `--metric loss` filters, `--points 50` adds resolution, `--json` is
  machine-readable. Read it while a run is live and after it ends, and pass the
  parent experiment's run too to compare curves, not just final numbers.
- `local`, `tinker`, `ssh` and `slurm` runs also get `TENSORBOARD_LOGDIR`, a
  per-run directory; pass it as the trainer's `logging_dir`/`log_dir`. `orx
  tensorboard <runId>` prints the command that opens it (not for SSH
  container runs).

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
| Vercel Sandbox (`vercel`) | [references/vercel.md](references/vercel.md) |
| OpenResearch (`openresearch`) | [references/openresearch.md](references/openresearch.md) |
| Tinker (`tinker`) | [references/tinker.md](references/tinker.md) |
| This machine (`local`) | [references/local.md](references/local.md) |

Do not read guides for backends you are not using. Kubernetes manifest work
always requires `references/k8s.md` before creating or editing the manifest.
If the installed reference cannot be read, `orx skill compute/<backend>` prints
the same canonical document.

For Slurm, `--cpus` and `--mem` request `#SBATCH --cpus-per-task` and
`#SBATCH --mem`; omitting either uses the configured Slurm default, then the
partition default. `--flavor cpu` requests no GPU even when the default flavor
is one. Read `references/slurm.md` before choosing values.

## Who watches a run

The OpenResearch server watches every run and catches up when reopened; jobs
keep running while it is closed. A run launched by orx 0.2 may still have a
detached supervisor process watching it; do not kill it, and the server takes
the run over once it exits.

## Results arrive in the chat

Launch ready runs with `orx exp run`, then end your turn. The launching chat is
subscribed automatically. Results wait until the chat is idle and may combine
several updates into one message. No waiting command or live agent session is
needed to keep the runs monitored. Silence does not mean a run failed. A run
can post an interim result by appending a line to
`$ORX_NOTIFY_FILE` (`>>`) when set; it arrives at most once per run per 10
minutes, with later lines coalesced.

When a result message arrives, read `orx runs <projectId>` and reconcile every
newly terminal run, including runs that finished while you were analyzing an
earlier result. Read the file located by `orx logs <runId>` and compare metrics
before deciding whether to repair, refill a slot, promote a result, or stop.
A failed run is not a new node: repair and relaunch the same experiment as
specified by `orx-experiment-tree`.

Attention messages also arrive when monitoring or cancellation needs action.
Inspect `orx exp status <expId>` and tell the user what needs
fixing. Cancel only a run that is broken or no longer wanted.

**Cancellation pending** means the cancel has been
accepted but has not reached the machine. It will be sent when the machine is
reachable again or the launch is confirmed. **Cancelling…** means the machine
is stopping the run. To stop tracking an unreachable run, use
`orx exp abandon <runId>`; double-check that the actual compute is shut down.
The cancel stays pending after Abandon.

Use `orx exp wake <expId>` to subscribe this chat to a run launched elsewhere.
`orx exp wait` remains available for humans; do not use it to keep an agent
session waiting for results.

## Sizing compute

- Decide GPU versus CPU first. API-driven evaluation and data preparation often
  run more cheaply on CPU.
- Pick the smallest shape that fits the model and a minimal batch.
- Escalate after a real OOM or hopelessly slow run instead of starting with the
  largest accelerator.
- Raise the timeout only for genuinely long runs.
