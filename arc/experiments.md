# Experiments and compute

orx makes each experiment reproducible and each result traceable to the exact code that produced it, wherever it ran. The agent may restart, the laptop may sleep and the dashboard may close while a job runs for days; none of that may lose a result or leave a job running unseen.

## The experiment tree

A project's experiments form a tree. A root is a **baseline**: starting code and its **run command**, the one shell command that trains or evaluates and prints results to the run log. Every other experiment is a child that inherits its parent's code and run command and changes one thing. A project may hold several baselines.

**Git holds the code; the store holds the tree.** An experiment's code is branch `orx/<slug>`, and a run records the exact commit it executed; parent links, notes, run commands and status live in the store. Re-parenting changes the tree and leaves the branches in place.

Agents are taught that a run's command and environment are a fixed contract inherited down the tree, and that an experiment whose run answered its question is frozen, so a recorded result always matches its branch (`agent-skills/orx-experiment-tree/`).

## A run is an immutable snapshot

**A backend never sees the project's history.** It gets one archive of the recorded commit, extracted into the run's own directory, and never clones the repository or touches an agent's worktree. Uncommitted edits do not ship.

1. `orx exp run` calls `POST /api/v2/runs` on this data directory's `orx up`, found through an owner-only descriptor and an HMAC hello, or started headless (`src/commands/remote_host.rs`). The CLI never launches a run itself.
2. `orx up` archives the experiment branch's commit (tar, plus zip for Ray) and stores it once under its SHA-256 (`src/compute.rs`).
3. It records the pending run, its snapshot identity and which chat asked.
4. The backend launches, and the handle it returns is saved on the run.

## Backends

Every backend implements one lifecycle: launch, recover a launch whose outcome is unknown, observe, cancel and serve logs (`Backend` in `src/engine/mod.rs`, adapters in `src/engine/adapters/`, provider clients in `src/jobs/`). `ComputeBackend` in `src/compute.rs` is the half before a run exists: preflight, staging and the launch spec.

Launching returns a **handle**, stored on the run with everything a later process needs to find the job again. Handles only ever grow; unknown stored data is preserved on rewrite or rejected, never dropped.

| Family | Runs on |
| --- | --- |
| this machine, Tinker | a detached `orx-host wrap` (a Job Object on Windows) |
| SSH, Slurm | the user's hosts, under `orx-host` ([remote](remote.md)) |
| Kubernetes, Ray | the user's cluster |
| Hugging Face Jobs, Modal, Vercel | hosted services |
| an OpenResearch box | a rented machine ([below](#managed-rentals)) |

A default flavor applies only to its own backend and never leaks onto an explicitly chosen one.

## The engine

**One engine per data directory, inside `orx up`.** The engine lock keeps it single ([storage](storage.md)). The dashboard, the desktop window, the CLI and agents are clients; none of them writes run state.

| Part | Role | Home |
| --- | --- | --- |
| Reconciler | Sole writer of run state; a fact and its notices commit in one transaction | `src/engine/reconcile.rs`, `src/engine/submit.rs` |
| Observers | One loop per host root, API account and rental, and one for this machine | `src/engine/runtime.rs` |
| Outbox | Delivers notices as hidden chat turns | `src/local/chat/` |
| Host link | One `ssh` per (host, root) to `orx-host serve` ([remote](remote.md)) | `src/engine/transport/` |

**The process that runs a job is never the engine.** A wrapper owns its payload and records the outcome in receipts on its machine; hosted services and rentals hold that state themselves. `orx up` may restart, sleep or update; it reconnects, reads what the job left and catches up. A headless `orx up` exits after a sustained idle period: no unfinished run, undelivered notice, active chat turn or client ([desktop](desktop.md) for the window).

[FROZEN.md](../crates/orx-host-proto/FROZEN.md) is the contract for host disk, the payload's environment, stored run state, API v2 and SSE, for the life of every queued job and lingering cleanup. Read it before changing any of them.

### Phase and contact

A run has two independent fields.

- **Phase** is the lifecycle: submitting, starting, running, done, failed, cancelled or lost.
- **Contact** is how fresh that knowledge is: live, unreachable, needs auth (SSH Connect or provider credentials), unreconciled (an outcome the engine cannot establish) or stopping (a cancelled run with processes left).

The invariants:

- **Phases move forward only**, and submitting leaves only by recording a handle or ending. Contact never moves the phase.
- **Never terminal on a timer.** A phase becomes terminal only on evidence: a receipt, a verified stop, a changed boot ID on the host, a scheduler's or provider's terminal state, a job proven gone, a launch proven absent after a cancel, or Abandon. No outage, backoff cap or silence ends a run.
- **Never silent.** Losing live contact raises an attention notice: at once for needs-auth and unreconciled, after a grace period for an outage, with a recovered notice when it heals. One outage yields one notice per chat.

### Submission

The run commits as submitting, with its launch spec, before any backend side effect; from then on recovery owns it. A launch spec never holds secrets: adapters add them at launch. An experiment holds one unfinished run unless the launch is forced.

A fence is written before the backend is called, and any launcher subprocess records itself on it, so a crashed launch's launcher is killed before anything retries. Outcomes:

- handle → starting
- rejected → failed
- never sent → stay submitting, with that contact; relaunch next tick
- reply lost → recover: found → starting; absent → relaunch once, then back off with the error shown; unknown → unreconciled, retried with backoff

**A launch is idempotent in the run ID.** On a host, the first launch intent is published exclusively; an identical retry adopts it, a different one conflicts, and the wrapper refuses a run that already started, so a payload never runs twice. API backends find a lost launch by the run ID where the provider allows it; elsewhere the run stays unreconciled until the user abandons it.

### Cancel and Abandon

Cancelling records the intent and reason on the run. Before the fence it is immediate; after it, the backend's stop proof decides the result, and a death seen while that verdict is pending does not end the run. After an accepted cancel, a job proven gone ends cancelled, and so does a nonzero exit or signal on a hosted service, keeping the reported exit code.

The engine delivers the stop once per connection, whenever the machine is reachable; the run's wrapper performs it, so verification never races its own exit ([remote](remote.md)). A run completes when its direct child exits; processes left behind are listed, raise one notice and stop only on request.

| State | Meaning |
| --- | --- |
| cancelling, while submitting, unreachable or needs auth | The cancel has not reached the machine. |
| cancelling, otherwise | The stop is running. |
| cancelled, live | The stop finished and a complete scan found nothing left. |
| cancelled, stopping | Survivors outlived the stop; the wrapper keeps killing them. |
| cancelled, live, `cancelUnconfirmed` set | The stop finished, but some processes could not be checked. |

`ui/src/runDisplay.ts` builds the dashboard's labels from these fields.

**Abandon** ends an unfinished run with no live contact as lost and sends the terminal notice. The kill stays pending: a terminal run without live contact is a **tombstone**, still watched, whose stop is delivered whenever the job or host returns, and which settles on the next proof that nothing is left.

### Logs

A run's log is written on the machine that ran it. The engine pages it from the host by byte range and file identity; a changed identity resets the reader rather than splicing different bytes. API backends offer no random access, so a running job's log streams into a local spool whether or not anyone is reading it. A stream that goes quiet is reopened, and resumes from a position that holds however the provider rechunks or cuts short its replay: a timestamp, a log entry, a byte offset. The spool outlives `orx up`: a restart continues it from that position, saved beside it after the bytes it covers, or from the spool's length when the provider replays the whole log. A spool without that file beside it came from an older orx and is rebuilt from the provider.

When a run ends, keeping its log becomes an obligation recorded in the same transaction as the terminal phase (`src/engine/runtime.rs`):

- A log within a size limit is kept whole; a larger one keeps its head and tail, where setup and results live, and the middle stays readable on the host.
- Collection resumes from its last synced offset and never gives up: it backs off, raises one notice after repeated failures, and retries at once when the host returns or the engine restarts.
- A backend releases provider resources only after the output is kept.
- A kept range claims end-of-file only if it reached the host's length.
- A page neither kept nor reachable answers 503, never an empty page.

Remote run directories go later, in bounded batches, once a run has settled and its log is kept (`src/local/remote_cleanup.rs`).

### Notices

**Results reach chats as messages.** The chat that launched a run is subscribed implicitly; another subscribes with `orx exp wake`. Every terminal phase and attention event writes one notice per subscribed chat in the reconciler's transaction, and the outbox delivers them as one hidden turn when the chat is idle, never interrupting a turn or a queued message. The one writer outside the engine is `orx exp wake` on a run that already finished, other than cancelled, which queues the terminal notice itself; the dedupe key keeps that to one.

- **Exactly once.** A batch's membership and turn ID are fixed before sending, so a resend after a crash is recognised as the same turn.
- **Never blocking.** Transient failures retry within a bounded window; a permanent one (the chat deleted) retires the batch, and its error shows on the run.
- **The payload can speak.** New bytes in `$ORX_NOTIFY_FILE` become rate-limited notices (`src/engine/notify.rs`); hosted providers have no such file. Notice text is data for the agent, never instructions.

Agents are taught to end their turn after launching: a blocking wait would die with the agent process, and a queued message survives it.

### Runs from 0.2.x

orx 0.2 and later builds with the engine switched off launched runs under a detached `orx supervise`; orx no longer launches that way. Such a run stays with its supervisor until it ends; the engine only reads it (`src/engine/legacy.rs`). Per-run supervisor locks decide who may write ([storage](storage.md)). Once a run has gone quiet for a settle period and its locks are free, the engine adopts it, keeping its original descriptor and stop semantics. A launch that died before recording its job takes the handle from its recovery file, or fails as its supervisor would have failed it.

The engine imports each 0.2.x wake-up once, including ones accepted after a downgrade or while it runs.

## Results

A run's results are what its command prints to the run log and what it logs to Trackio (`src/local/trackio.rs`); agents read metrics through `orx metrics`. Figures and reports land in the artifacts folder, and the run's commit ties each number to its code. orx adds no interpretation of its own.

## Managed rentals

A run on an OpenResearch box rents a machine through openresearch.sh for that run alone. **The laptop sleeping, losing its network or quitting never decides when billing stops.** A box is an `orx-host` root like an SSH host, staged through the same transport, with the same receipts and log collection ([remote](remote.md)); billing duty is the one addition.

1. orx creates the box with a rental lease and stages source over SSH. While provisioning and staging, the laptop heartbeats the rental, within a bounded preparation budget.
2. An owner-only file on the box holds a capability scoped to that rental; it never appears in API responses.
3. `orx-host wrap` activates the rental, starting billing, before it starts the payload, then heartbeats on its own thread; the service expires a rental whose heartbeats stop. The run stays starting until the payload starts.
4. After the exit receipt, the wrapper waits for the laptop to acknowledge that it kept the log and final notices, or a bounded grace, then stops heartbeating and retries release until the service accepts. The lease adds that grace to the payload's time limit, up to the service's maximum, so a run near the maximum has less time to be collected. Live contact makes pending collection due at once.

The rules around it (`crates/orx-host/src/rental.rs`, `src/engine/adapters/openresearch.rs`, `src/jobs/rental.rs`):

- **An uncertain start is never repeated;** the engine inspects the run's intent on the box instead.
- **Cancel and Abandon release the rental** from the laptop too, so stopping billing never waits on the box. A successful release is stop proof: Cancel ends cancelled, Abandon ends lost with no kill pending.
- **A box that never comes online fails the run.** A box the service reports failed, offline or dead, or deletes before it is online, ends the run failed with the service's reason. One still provisioning when the preparation budget runs out is released, and the release ends the run failed.
- **A rejected key ends the run.** A box the engine never reached during the run launched nothing, so once its key refusals open the breaker the engine releases the rental and fails the run, naming the cause: the box lacks the registered key, no registered key is on this computer, or the local key needs a passphrase no agent supplies. `orx exp run` refuses that last one before creating a box, and only warns on macOS, where the keychain may supply it. After a box reachable during this run rejects its SSH key continuously for a grace period, the engine issues Cancel if the companion API shows the rental active. Connect or live contact resets the clock; network outages never qualify. The timer only requests cancellation: release proof makes the run terminal, and an already released rental settles as lost.
- **No research data crosses this protocol.** Logs, commands, source, experiment IDs, outcomes and artifacts never reach the billing service.
- **Collection is bounded by the box's life.** Once release is verified, the local copy is the kept log, even if empty. A run whose final outcome went with the machine ends as lost, with the service's reason when it reports the box failed or dead.
- **Rentals from 0.2 stay `Handle::Legacy`.** The engine inspects their run directory on the box and collects their logs; it never launches on them.
