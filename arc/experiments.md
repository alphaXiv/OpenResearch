# Experiments and compute

Research is a sequence of guesses tested by running code. orx's job is to
make each test reproducible and each result traceable to the exact code that
produced it, wherever the code ran: this machine, a GPU box over SSH, a
cluster, a hosted job service. The agent proposing the guesses may be
restarted, the laptop may sleep, and the dashboard may be closed while a job
runs for hours. None of that may lose a result or leave a job running unseen.

## The experiment tree

A project's experiments form a tree. The root is a **baseline**: the starting
code and its **run command**, the one shell command that trains or evaluates
and prints results to the run log. Every other experiment is a child that
inherits its parent's code and run command and changes one thing. A project
may hold more than one baseline.

```text
store                                   project repository (Git)
experiment: parent, title, notes,  ───► branch orx/<slug>: the code
            run command, status
run: experiment, commit, backend   ───► the exact commit the run executed
     descriptor, status
```

**Git holds the code; the store holds the tree.** Each experiment owns a
branch in the project's repository, and each run records the commit it
executed, so any result can be checked out, diffed against its parent, or
pushed with ordinary Git. The store holds the parent links, titles, notes,
the run command and status. Re-parenting an experiment changes the tree and
leaves the branches where they are.

Agents are taught two rules that make results comparable: a run's command and
environment are a fixed contract inherited down the tree, and an experiment
whose run has answered its question is frozen. Further changes go on a child,
so a recorded result always matches the code on its branch.

## Choosing the run engine

New experiment engine in Compute and Settings opts new runs into the engine being
tested; it defaults to off on stable and dev builds. `ORX_RUN_ENGINE=1` or `0`
in the `orx up` server's environment overrides the saved `runEngine` setting.
Canary builds always use the engine, ignoring the setting and environment.
Both launch APIs make the same choice. With it off, the backend launchers and
detached `orx supervise` processes launch and watch runs as in 0.2.x. The
engine still runs, importing their results and wakeups and recovering engine
runs already in flight. Existing owners stay in place; the setting chooses
which path recovers orphaned supervised runs.

Engine runs keep their first payload start in `started_at` and finish in
`ended_at`, using source timestamps when available and observation time otherwise.

The snapshot contract below applies in both modes. The engine lifecycle and
`orx-host` sections describe engine-owned runs; supervised runs keep their
backend descriptors and stop semantics.

## A run is an immutable snapshot

```text
orx exp run ─► POST /api/v2/runs to this data directory's `orx up`
            ─► archive the experiment branch's commit (tar, plus zip for Ray)
            ─► name the archive by its SHA-256, store it once
            ─► record the pending run and its snapshot identity
            ─► backend launch ─► handle saved on the run
```

**A backend never sees the project's history.** A launch archives the
recorded commit once, names the archive by its content hash, and hands that
payload to the backend, which extracts it into the run's own directory. A
backend never clones the repository and never touches an agent's worktree.
The run executes exactly what was committed: uncommitted edits do not ship,
and two runs of the same commit share one archive.

The CLI never launches a run itself. Inside a chat it reaches the dashboard
that owns the chat, which records which chat asked; elsewhere it finds the
running `orx up` through an owner-only descriptor and an HMAC hello, or starts
a headless one.

## Backends

Every backend implements one lifecycle: launch, recover a launch whose outcome
is unknown, observe, cancel, and serve logs. Launching returns a **handle**,
stored on the run with everything a later process needs to find the job
again: a host and run directory, a Slurm job, an API account and the resources
the run owns, primary job first. Handles only ever grow; unknown stored data
is preserved on rewrite or rejected, never dropped.

The families differ in where the job runs and how the source gets there:

| Family | Runs on | Source travels |
| --- | --- | --- |
| this machine, Tinker | a detached `orx-host wrap` (a Job Object on Windows) | extracted locally |
| SSH, Slurm | the user's own hosts, under `orx-host` ([remote](remote.md)) | a content-addressed upload, resumable |
| Kubernetes | the user's cluster | copied into the pod by `kubectl` |
| Ray | the user's Ray cluster | the job's working-directory package |
| Hugging Face Jobs, Modal, Vercel | hosted services | a private job volume, the sandbox filesystem |
| an OpenResearch box | a rented machine | a tar stream over SSH |

Which backend a launch uses comes from the command line, else the user's
default compute target. A default flavor applies only to its own backend and
never leaks onto an explicitly chosen one.

## The engine

**One engine per data directory, inside `orx up`.** The dashboard, the
desktop window, the CLI and agents are all clients of it; none of them writes
run state.

```text
dashboard, app, orx CLI, agents ─HTTP + SSE─► orx up   (engine lock: exclusive)
                                              ├─ reconciler   sole writer of run state and notices
                                              ├─ observers    one per host root or API account
                                              ├─ outbox       notices ─► hidden chat turns
                                              └─ one ssh per (host, root) ─► orx-host serve
                                                                             └─ orx-host wrap, one per run
```

**The process that runs a job is never the engine.** Each run's wrapper owns
its payload and records the outcome in receipts on the machine that ran it.
`orx up` may restart, sleep or update; it reconnects, reads the receipts and
catches up. A headless `orx up` exits only after ten idle minutes with no
unfinished run, no undelivered notice and no client
([desktop](desktop.md) for the window).

### Phase and contact

A run has two independent fields. **Phase** is the lifecycle: submitting,
starting, running, done, failed, cancelled or lost. **Contact** is how fresh
that knowledge is: live, unreachable, needs auth (SSH Connect or provider
credentials), unreconciled (an outcome the engine cannot establish), or stopping
(a cancelled run with processes left). A non-live contact marks the shown phase
stale with its `since`.

- **Phases move forward only**, and submitting leaves only by recording a
  handle or ending. Contact never moves the phase.
- **Never terminal on a timer.** A phase becomes terminal only on evidence: a
  receipt, a verified stop, a changed boot ID on the host, a scheduler's
  terminal state, a launch proven absent after a cancel, or Abandon. No
  outage, backoff cap or silence ends a run; jobs run for days.
- **Never silent.** Losing live contact raises an attention notice to the
  run's chats: at once for needs-auth and unreconciled, after ten minutes for
  an outage, with a recovered notice when it heals. One outage yields one
  notice per chat.

### Submission

The run is committed as submitting, with its launch spec, before any side
effect; from then on recovery owns it. A fence is written before the backend
is called, and any launcher subprocess records itself on it, so a crashed
launch can be found and its launcher killed before anything retries.

```text
launch ─► handle            → starting
       ─► rejected          → failed
       ─► never sent        → stay submitting, with that contact; relaunch next tick
       ─► reply lost        → recover: found → starting
                                       absent → relaunch once, then back off with the error shown
                                       unknown → unreconciled, retried with backoff
```

**A launch is idempotent in the run ID.** On a host, the first launch intent
is published exclusively; an identical retry adopts it, a different one is a
conflict, and the wrapper refuses to start if the run has already started. A
payload never runs twice. API backends find a lost launch by a label carrying
the run ID where the provider allows it; where it does not, the run stays
unreconciled until the user abandons it.

### Cancel and Abandon

Cancelling records the intent and its reason on the run. For a run on a hosted
service, a nonzero exit or signal after an accepted cancel ends Cancelled,
preserving the reported exit code. The engine delivers the stop whenever the
run's machine is reachable, and the run's wrapper performs the stop, so
verification never races its own exit: a container's cancel hook, TERM, a
grace period, KILL, then fresh scans of every process carrying the run's tag.
Completion of a run is its direct child's exit; processes it left behind are listed, raise one notice, and are stopped only on request.

| State | Meaning |
| --- | --- |
| Cancellation pending | The cancel has not reached the machine. Abandon is offered, with a warning to double-check that the actual compute is shut down. |
| Cancelling… | The stop is running. |
| Cancelled | The stop finished and a complete scan found nothing left. |
| Not fully stopped · N processes still running on *host* | Survivors outlived the stop; the wrapper keeps killing them, and Stop now retries. |
| Cancelled · couldn't confirm | Rare: the stop succeeded but some processes could not be checked, as inside a container. |

**Abandon** ends an unfinished run that has no live contact as lost, and sends
its chats the terminal notice. The kill stays pending: a terminal run without
live contact is a **tombstone**, still watched, whose stop is delivered
whenever the job or host comes back, and which settles on the next proof that
nothing is left.

### Logs

A run's log is written on the machine that ran it. While someone watches, the
engine follows its length and pages it from the host by byte range and file
identity; a changed identity resets the reader rather than splicing different
bytes. API backends offer no random access, so their logs stream into a local
spool.

Kubernetes cancellation keeps the deleting Job visible until its Pods are gone.
Once both are gone, the log already spooled on this computer becomes the kept log.

When a run ends, keeping its log becomes an obligation recorded in the same
transaction as the terminal phase:

- A log up to 1 GiB is kept whole; a larger one keeps its first 64 MiB and its
  last 256 MiB, where setup and results live. The middle stays readable from
  the host until its run directory is cleaned up.
- Collection resumes from its last synced offset and never gives up: it backs
  off to hourly, raises one notice after repeated failures, and retries at
  once when the host returns.
- A kept range claims end-of-file only if its last page reached the host's
  length, so a log that grew afterwards is served on from the host.
- A page the laptop does not hold, from a host that is down, answers "Page not
  on this computer", and the viewer retries when the host returns.

### Notices

**Results reach chats as messages.** The chat that launched a run is
subscribed implicitly; another chat subscribes with `orx exp wake`. Every
terminal phase and attention event writes one notice row per subscribed chat
in the reconciler's transaction, and the outbox delivers them as one hidden
turn when the chat is idle, never interrupting a turn or a queued message.

- **Exactly once.** A batch's membership is frozen and its turn ID fixed
  before sending, so a resend after a crash is recognised as the same turn.
- **Never blocking.** Transient failures retry for a day; a permanent one (the
  chat deleted) retires the batch at once, and its error shows on the run.
- **The payload can speak.** A job appends free text to `$ORX_NOTIFY_FILE`;
  new bytes become at most one notice per run per ten minutes. Notice text is
  data for the agent, never instructions.

Agents are taught to launch, end their turn, and reconcile with `orx runs`
when a message arrives. A blocking wait would hold a turn open for days and
die with the agent process; a queued message survives both.

### Runs from 0.2.x

A run started under a detached `orx supervise` stays with that supervisor
until it ends; the engine only reads it. With the setting off, `orx up`
re-spawns missing supervisors at startup and periodically. With it on, only
the engine adopts orphans, after the run has had no update for five minutes
and its supervisor locks are free. Adopted runs keep their original
descriptors and stop semantics. Changing the setting leaves engine-owned
runs and live supervisors in place.

The engine imports each 0.2.x wake-up subscription once, including subscriptions
accepted after a downgrade or while the engine is running.

Remote run directories are cleaned up later in bounded batches per host,
keeping each project's newest finished runs and the source archives live runs
still need.

The on-disk and wire formats above are frozen for the life of every queued
job and lingering cleanup: [FROZEN.md](../crates/orx-host-proto/FROZEN.md).

## Results

A run's results are what its command prints to the run log and what it logs
to Trackio, which can live in a Hugging Face Space, on a server, or locally,
keyed by project and run. Agents read metrics through `orx metrics`, figures
and reports land in the project's artifacts folder, and the run's commit ties
each number to its code. orx stores no interpretation of a result; the run
log, the metrics and the code are the evidence.

## Managed rentals

A run on an OpenResearch box rents a machine through openresearch.sh for that
run alone. The rental is designed so that the user's laptop sleeping, losing
its network, or quitting never decides when billing stops.

```text
laptop                         rented machine                  openresearch.sh
stage source + payload  ─ssh─► rental-supervise (detached)
                               activate ─────────────────────► billing starts
                               heartbeat, every 30 s ────────► (expires without)
                               run payload, log, exit code
collect log + exit code ◄─ssh─ wait for "collected", 60 s max
                               release ──────────────────────► billing closes,
                                                               machine deleted
```

- The laptop stages the source and command directly over SSH. An owner-only
  file on the machine gives a detached `orx rental-supervise` a capability
  scoped to that one rental; the capability never appears in API responses.
- The remote supervisor runs the payload under a hard time limit in its own
  process group, writes the log and an atomically renamed exit code, and
  heartbeats the service while it works. A launch whose start is uncertain is
  never retried; its rental expires instead.
- After the payload exits, the supervisor waits for the laptop to acknowledge
  that it saved the final log and exit code, or 60 seconds, whichever comes
  first, then releases the rental, retrying until the service accepts. The
  service closes billing and deletes the machine on its own schedule.
- **Cancel and Abandon release the rental** from the laptop as well, so
  stopping billing never waits on the box being reachable. A successful
  release is stop proof: Cancel ends Cancelled, Abandon ends Lost with no kill
  pending. After a box that was reachable during this run rejects its SSH
  key continuously for 15 minutes, the engine checks the companion API and
  issues Cancel if the rental is still active; Connect or Live contact resets
  the clock, and network outages never qualify.
  The timer requests cancellation, while release proof makes the run terminal;
  an already released rental settles as Lost without another release.
- The engine drives the box over its own SSH connection, so first contact,
  backoff and the auth breaker ([remote](remote.md)) apply to it.
- **No research data crosses this protocol.** Logs, commands, source,
  experiment IDs, outcomes and artifacts never reach the billing service.
- Manual teardown follows the run engine's Cancel and Abandon path above.
- Collection is best effort. Data left only on the machine is lost when it is
  deleted, and a run whose final outcome could not be collected ends as
  **lost** ("Outcome unavailable") instead of running forever. The chat is
  told not to assume success or failure.
