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

## A run is an immutable snapshot

```text
orx exp run ─► archive the experiment branch's commit (tar, plus zip for Ray)
            ─► name the archive by its SHA-256, store it once
            ─► backend: preflight ─► submit ─► descriptor saved on the run
            ─► spawn a detached `orx supervise`
```

**A backend never sees the project's history.** A launch archives the
recorded commit once, names the archive by its content hash, and hands that
payload to the backend, which extracts it into the run's own directory. A
backend never clones the repository and never touches an agent's worktree.
The run executes exactly what was committed: uncommitted edits do not ship,
and two runs of the same commit share one archive.

When an agent runs `orx exp run` inside a chat, the command hands the launch
to the dashboard that owns the chat, which submits it and records which chat
asked. Without a dashboard, the CLI launches the run itself.

## Backends

Every backend implements one lifecycle: preflight (can this run start here?),
stage the source, submit, then status, logs, cancel and cleanup. Submission
returns a **backend descriptor**, a serialized handle stored on the run with
everything a later process needs to find the job again: a job ID, a host and
directory, a Kubernetes context, a rental capability. The descriptor's fields
only ever grow. A field a backend needs must exist on the descriptor, because
a parse and reserialize drops anything it does not name.

The families differ in where the job runs and how the source gets there:

| Family | Runs on | Source travels |
| --- | --- | --- |
| this machine | a detached process | extracted locally |
| SSH, Slurm | the user's own hosts | a tar stream over SSH |
| Kubernetes | the user's cluster | copied into the pod by `kubectl` |
| Ray | the user's Ray cluster | the job's working-directory package |
| Hugging Face Jobs, Modal | hosted services | a private job volume, the sandbox filesystem |
| an OpenResearch box | a rented machine | a tar stream over SSH |
| Tinker | a local controller using remote model compute | extracted locally |

Which backend a launch uses comes from the command line, else the user's
default compute target. A default flavor applies only to its own backend and
never leaks onto an explicitly chosen one.

## The supervisor

**The process that watches a job is never the dashboard.** Each launch spawns
a detached `orx supervise` for the run. It polls the backend, tails the job's
output into the run's log file ([storage](storage.md)), records status
transitions, and honors cancellation. Closing the dashboard, quitting the app
or restarting it for an update leaves every job watched.

- **One supervisor per run.** A per-run file lock makes a second supervisor
  exit at once.
- **Restart-idempotent.** All of a supervisor's state is the store, the
  backend and the log file. A new supervisor reattaches from the descriptor
  and replays the backend's log over the local copy, truncating only once
  replay begins, so a supervisor started while the backend is unreachable
  keeps what is already there. The dashboard starts a supervisor for any
  submitted, active run that has none.
- **Statuses move forward only.** A run is starting, running, done, failed,
  cancelled or lost. An active run finishes once, and a terminal outcome never
  changes, however late a stale update arrives.
- **Cancellation is recorded locally and honored remotely.** Cancelling writes
  the intent and its reason to the run; the supervisor sees it and stops the
  job through the backend. The intent survives if the backend is briefly
  unreachable.
- **Monitoring trouble is reported, not hidden.** A supervisor that cannot
  reach its backend for a while records that on the run, and chats waiting on
  the run hear about it.

When a run ends, the chats that asked to be woken for it get a wake-up turn
with the outcome. Asking is explicit (`orx exp wake`), and the wake-ups are
rows in the store with a claim and a TTL, so a crash between claiming and
delivering never loses or duplicates one, and a wake-up never interrupts a
turn already running or a message the user queued.

Remote run directories are removed later by a background pass, in bounded
batches per host, which keeps the source archives that live runs still need.

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
- **No research data crosses this protocol.** Logs, commands, source,
  experiment IDs, outcomes and artifacts never reach the billing service.
- Collection is best effort. Data left only on the machine is lost when it is
  deleted, and a run whose final outcome could not be collected ends as
  **lost** ("Outcome unavailable") instead of running forever. The chat is
  told not to assume success or failure.
