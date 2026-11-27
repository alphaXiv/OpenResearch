# Experiments and compute

orx makes each experiment reproducible and each result traceable to the exact code that produced it, wherever it ran. The agent may restart, the laptop may sleep and the dashboard may close while a job runs for days; none of that may lose a result or leave a job running unseen.

## The experiment tree

A project's experiments form a tree. A root is a **baseline**: starting code and its **run command**, the one shell command that trains or evaluates and prints results to the run log. Every other experiment is a child that inherits its parent's code and run command and changes one thing. A project may hold several baselines.

**Git holds the code; the store holds the tree.** An experiment's code is branch `orx/<slug>`, and a run records the exact commit it executed; parent links, notes, run commands and status live in the store. Re-parenting changes the tree and leaves the branches in place.

Agents are taught that a run's command and environment are a fixed contract inherited down the tree, and that an experiment whose run answered its question is frozen, so a recorded result always matches its branch (`agent-skills/orx-experiment-tree/`).

## A run is an immutable snapshot

**A backend never sees the project's history.** It gets one archive of the recorded commit, extracted into the run's own directory, and never clones the repository or touches an agent's worktree. Uncommitted edits do not ship.

1. `orx exp run` calls `POST /api/v2/runs` on this data directory's `orx up`, found through an owner-only descriptor and an HMAC hello, or started headless (`src/commands/remote_host.rs`). The CLI never launches a run itself.
2. `orx up` archives the experiment branch's commit and stores it once under its SHA-256 (`src/compute.rs`).
3. It records the pending run, its snapshot identity and which chat asked.
4. The backend launches, and the handle it returns is saved on the run.

## Backends

Every backend implements one lifecycle: launch, recover a launch whose outcome is unknown, observe, cancel and serve logs (`Backend` in `src/engine/mod.rs`, adapters in `src/engine/adapters/`, provider clients in `src/jobs/`). `ComputeBackend` in `src/compute.rs` is the half before a run exists: preflight, staging and the launch spec.

Launching returns a **handle**, stored on the run with everything a later process needs to find the job again. Handles only ever grow; unknown stored data is preserved on rewrite or rejected, never dropped.

| Family | Runs on |
| --- | --- |
| this machine, Tinker | a detached `orx-host wrap` |
| SSH, Slurm | the user's hosts, under `orx-host` ([remote](remote.md)) |
| Kubernetes, Ray | the user's cluster |
| Hugging Face Jobs, Modal, Vercel, boxd | hosted services |
| an OpenResearch box | a rented machine ([below](#managed-rentals)) |

A default flavor applies only to its own backend and never leaks onto an explicitly chosen one.

### The price of a run

**A run is priced once, when its job exists.** The engine asks the backend what it charges (`Backend::quote`) and stores the answer on the run; a later price change never rewrites it, and reading it never holds up the launch. Spend is the quote times the run's running time, so a running run's is an estimate. **What the provider billed replaces the estimate** once the provider reports it, since a quote cannot know what a run used; a list-priced run that finished unpriced is priced at the next start. A backend that bills but could not be read shows its cost as unavailable; the user's own machines and services with no readable price show none.

## The engine

**One engine per data directory, inside `orx up`.** The engine lock keeps it single ([storage](storage.md)). The dashboard, the desktop window, the CLI and agents are clients; none of them writes run state.

| Part | Role | Home |
| --- | --- | --- |
| Reconciler | Sole writer of run state; a fact and its notices commit in one transaction | `src/engine/reconcile.rs`, `src/engine/submit.rs` |
| Observers | One loop per host root, API account and rental, and one for this machine | `src/engine/runtime.rs` |
| Outbox | Delivers run notices and flag wakes as hidden chat turns | `src/local/chat/` |
| Host link | One `ssh` per (host, root) to `orx-host serve` ([remote](remote.md)) | `src/engine/transport/` |

**The process that runs a job is never the engine.** A wrapper owns its payload and records the outcome in receipts on its machine; hosted services and rentals hold that state themselves. `orx up` may restart, sleep or update; it reconnects, reads what the job left and catches up. A headless `orx up` exits after a sustained idle period: no unfinished run, undelivered notice, active chat turn or client ([desktop](desktop.md) for the window).

A local run keeps its machine from idle sleep while it runs, as a playing video does; closing the lid still sleeps it (`crates/orx-host/src/awake.rs`).

[FROZEN.md](../crates/orx-host-proto/FROZEN.md) is the contract for host disk, the payload's environment, stored run state, API v2 and SSE, for the life of every queued job and lingering cleanup. Read it before changing any of them.

### Phase and contact

A run has two independent fields.

- **Phase** is the lifecycle: submitting, starting, running, done, failed, cancelled or lost.
- **Contact** is how fresh that knowledge is: live, unreachable, needs auth, unreconciled (an outcome the engine cannot establish) or stopping (a cancelled run with processes left).

A run that has not started also keeps the backend's latest word on why it waits, shown beside its status.

The invariants:

- **Phases move forward only**, and submitting leaves only by recording a handle or ending. Contact never moves the phase.
- **Never terminal on a timer.** A phase becomes terminal only on evidence: a receipt, a verified stop, a changed boot ID on the host, a scheduler's or provider's terminal state, a job proven gone, a launch proven absent after a cancel, or Abandon. No outage, backoff cap or silence ends a run.
- **Never silent.** Losing live contact raises an attention notice, and healing it a recovered one; one outage yields one notice per chat.

### Submission

The run commits as submitting, with its launch spec, before any backend side effect; from then on recovery owns it. A launch spec never holds secrets: adapters add them at launch. An experiment holds one unfinished run unless the launch is forced.

A fence is written before the backend is called, and any launcher subprocess records itself on it, so a crashed launch's launcher is killed before anything retries. A launch then ends in a handle, a rejection, or an unknown outcome that recovery resolves by asking the backend, never by guessing (`src/engine/submit.rs`).

**A launch is idempotent in the run ID.** On a host, the first launch intent is published exclusively, an identical retry adopts it, and the wrapper refuses a run that already started, so a payload never runs twice. API backends find a lost launch by the run ID where the provider allows it; elsewhere the run stays unreconciled until the user abandons it.

### Cancel and Abandon

Cancelling records the intent, its reason and who asked (the user, the chat's agent or a rule). The first accepted stop wins and is never replaced. Before the fence the cancel is immediate; after it, the backend's stop proof decides the result.

The run's wrapper performs the stop, delivered whenever the machine is reachable, so verification never races its own exit ([remote](remote.md)). A run completes when its direct child exits; processes left behind are listed, raise one notice and stop only on request. `ui/src/runDisplay.ts` turns phase, contact and cancel state into the dashboard's labels.

**Abandon** ends an unfinished run with no live contact, or whose stop its backend keeps refusing, as lost and sends the terminal notice. The kill stays pending: a terminal run without live contact is a **tombstone**, still watched, whose stop is delivered whenever the job or host returns.

### Logs

A run's log is written on the machine that ran it ([storage](storage.md)). The engine pages it from the host by byte range and file identity, so a replaced log is never spliced onto the old one. API backends offer no random access, so a running job's log streams into a local spool that survives restarts and resumes where the provider's replay allows.

**A run's end records its log debt in the same transaction as its terminal phase** (`src/engine/runtime.rs`). Collection keeps the log, or its head and tail when it is large, resumes from its last offset, and never gives up: it backs off, says so once, and retries when the host returns. A backend releases provider resources only after the output is kept, and a page neither kept nor reachable is an error, never an empty page. Remote run directories are cleaned up only after their log and signals are collected (`src/local/remote_cleanup.rs`).

### Notices

**Results reach chats as messages.** The chat that launched a run is subscribed implicitly; another subscribes with `orx exp wake`. Every terminal phase and attention event writes one notice per subscribed chat in the reconciler's transaction, and the outbox delivers them as one hidden turn when the chat is idle, never interrupting a turn or a queued message, and never right after a Stop ([agents](agents.md)).

- **Exactly once.** A batch's membership and turn ID are fixed before sending, so a resend after a crash is recognised as the same turn.
- **Never blocking.** Transient failures retry within a bound; a permanent one (the chat deleted) retires the batch, and its error shows on the run.
- **The payload can speak.** New bytes in `$ORX_NOTIFY_FILE` become rate-limited notices (`src/engine/notify.rs`). Notice text is data for the agent, never instructions.

Agents are taught to end their turn after launching: a blocking wait would die with the agent process, and a queued message survives it.

### Runs from 0.2.x

orx 0.2 launched runs under a detached `orx supervise`; orx no longer launches that way. Such a run stays with its supervisor until it ends and its supervisor locks are free ([storage](storage.md)), and the engine only reads it until then; afterwards it adopts the run, keeping its original descriptor and stop semantics (`src/engine/legacy.rs`).

## Results

A run's results are what its command prints to the run log and what it logs to Trackio (`src/local/trackio.rs`); agents read metrics through `orx metrics`. Figures and reports land in the artifacts folder, and the run's commit ties each number to its code. orx adds no interpretation of its own.

## Managed rentals

A run on an OpenResearch box rents a machine through openresearch.sh for that run alone. **The laptop sleeping, losing its network or quitting never decides when billing stops.** A box is an `orx-host` root like an SSH host, staged through the same transport, with the same receipts and log collection ([remote](remote.md)); billing duty is the one addition.

1. orx creates the box with a rental lease and stages source over SSH, heartbeating the rental while it prepares.
2. An owner-only file on the box holds a capability scoped to that rental; it never appears in API responses.
3. `orx-host wrap` activates the rental, starting billing, before it starts the payload, then heartbeats on its own; the service expires a rental whose heartbeats stop.
4. After the payload exits, the wrapper waits a bounded grace for the laptop to keep the log and final notices, then releases the rental. The lease reserves that grace beyond the run's time limit, up to the service's maximum, so a run near the maximum has less time to be collected.

The rules around it (`crates/orx-host/src/rental.rs`, `src/engine/adapters/openresearch.rs`, `src/jobs/rental.rs`):

- **An uncertain start is never repeated;** the engine inspects the run's intent on the box instead.
- **Cancel and Abandon release the rental** from the laptop too, so stopping billing never waits on the box. A successful release is stop proof.
- **A box that never becomes usable ends the run.** One the service reports failed, or one that never comes online or never accepts the user's key, is released and the run fails naming the cause.
- **No research data crosses this protocol.** Logs, commands, source, experiment IDs, outcomes and artifacts never reach the billing service.
- **Collection is bounded by the box's life.** Once release is verified, the local copy is the kept log, and an outcome that went with the machine ends the run as lost.

## Run signals

A payload may print structured progress and measures as `::orx::` lines (the vendored helper is `orx skill compute/assets/orx_run.py`), and orx also detects ordinary progress bars and counters; declared progress wins over detected. **Progress is advisory:** no measurement finishes a run or proves containment.

A **fold** (`crates/orx-signals/`) reads a log into a bounded **checkpoint** of progress, measures and tables. It runs beside the log, on the host for host runs and on the laptop over API spools, and either side can continue the other's checkpoint, so a restart resumes rather than rereads. Each collection pairs the host's clock with the laptop's, so clock skew never moves a run's timeline.

- **A checkpoint is never replaced by one that knows less.** A log folded again from its start is a rebuild, staged beside the published checkpoint until it covers as much (`src/engine/adapters/signals.rs`).
- **Unknown is not absent.** A checkpoint that cannot be read, or a kept file that is gone, is reported as such.
- **Signals are a debt of their own.** A finished run owes signal collection separately from its log; it settles complete, or incomplete with one notice saying why, and a run that reported nothing settles quietly.

### The monitor

A run's monitor is one projection (`src/engine/monitor.rs`) of its checkpoint, the estimator (`src/engine/estimate.rs`) and its lifecycle: the summary every `Run` carries, the whole monitor `GET /api/runs/:id/monitor` serves, and the text `orx exp status` prints, all at one `rev`. Only `orx up` publishes it, and `rev` only grows, across restarts too, so every client and reader agrees on one version of a run.

- **The estimate is computed once, on the server.** Pages only count it down.
- **An estimate promises only what the run declared.** Time left from a detected bar or counter is a floor, since more work may follow it; a run with no total fills against its time limit.
- **Cost comes only from what a run records,** never invented for a machine without a known rate.
- **Time the laptop did not watch is unknown.** A run whose collection is overdue reads as stale and its estimate stops moving.
- **A failing projection is said to every reader** until one succeeds, and the run stays watched meanwhile.

A run without a view of its own is drawn from what it reported. A **view** is markup the chat's agent writes on an experiment with `orx exp view set` (`src/engine/view.rs`): signal declarations and blocks of charts, text and tables. It is stored as written and derived on every read, a child experiment starts with a copy, and saving checks it against what the experiment's runs reported. A view changes what is shown, never what runs. For a log an agent cannot change, saved **patterns** read lines into records as if the run had printed them (`crates/orx-signals/src/patterns.rs`); an edit refolds each run's log from its start.

`orx exp view show` and `orx exp view check` describe to an agent, in text, what the dashboard draws and what each pattern matched. The watcher also checks a run's instrumentation once reporting starts and sends at most one notice naming what is missing; it never stops or retries the run.

A run compares with its parent experiment's latest successful run with signals, else its newest with signals, on each signal whose view says which way is better, at the same stage and step or at the end. Anything else is no comparison, never a level one.

### Rules

An agent declares rules beside an experiment's stage plan (`orx exp view set --config`, `src/engine/rules.rs`): each a bounded condition on what a run reports, never an expression, that stops the run or raises a flag. **A run freezes the rules it launched with,** so an edit changes only later runs.

Only `orx up` checks them, and only on a live run with a current, caught-up checkpoint. What each rule counted and that it acted commit with any flag or cancel in one transaction, so a restart neither counts a report twice nor acts twice. A rule on the estimate acts only on a settled pace. A stop goes through the engine's cancel, with no model involved.

A stopped run reads **Revising** while the chat's agent repairs its experiment (`orx exp revise`). The server derives that state from the agent's turn and the experiment's next run, never stores it, and nothing claims the repair happened: the next run is the proof.

### The watcher

One cheap model per chat watches the chat's runs and writes each run's sentence (`src/engine/watch.rs`). Which model is one function's answer: the user's pick for the chat, else the agent's own cheap model, the chat's own model when it is free, or the vendor's cheap model through the chat's provider or the user's own Claude Code or Codex. A chat with none waits for a pick, which its status report offers once open. **A look has no tools:** a harness that cannot promise that is not used.

- **A look happens when something changes** (a trend breaks, a stage starts or ends, the log prints a new kind of warning), at growing intervals while the run keeps printing, and once more when it ends. A quiet run costs nothing.
- **A look sees a bounded digest, never the whole log or the chat** (`src/engine/watch/digest.rs`). Every field passes sharing's credential scan before it is sent ([storage](storage.md)).
- **The model never writes a number.** Its reply is strict JSON whose figures are tokens orx fills from the run, and a reply that fails the checks keeps the run's last sentence (`src/engine/watch/reply.rs`).
- **A restart never replays a sent look.** It is recorded before it is sent, and one cut short by a restart is marked interrupted. A look may make one extra call to correct a reply that failed its checks.

**The status report reads the chat.** The same model keeps one Markdown report per chat for a person returning to a long effort, edited in place rather than appended to (`src/engine/watch/report.rs`). A chat's report opens once the chat goes on working after one of its runs ends, by launching another run or by working a minute past the end, and its first update waits for that turn to end; a literature review or a launch alone never opens one. The user can turn it on at once, off for that chat, or off for new chats, which stops it opening on its own. Once open it updates at milestones: the end of a turn that reacted to run results or launched a run, or the agent's `orx milestone`, which does not wait for the turn. The model edits the report rather than rewriting it, and writes it again shorter once it outgrows the open card. Every version is kept. Updates keep the watcher's least gap, and a milestone due when orx stops is still due when it starts. Unlike a look, an update sees the conversation since the last one beside the runs, notices and the agent's notes, each field through the same credential scan. Its words are the model's, but every number must appear in what it was shown: a reply that fails gets one correction, and otherwise the last report stands with the reason beside it.

### Flags

A flag says a run may be going wrong and **never stops it** (`src/engine/flags.rs`). A look, a rule that asks, or orx's own tripwires raise one; tripwires watch every live run, watched by a model or not, for values that are not numbers, a signal far worse than the parent's, stalls, silence and new kinds of errors (`src/engine/watch/trip.rs`). A flag keeps the log lines it cites.

- **One trouble, one flag.** A run has at most one open flag from a look or a tripwire, and later looks note under it. Keep going settles only that run's flag: a later run with the same cause is flagged again, since a missed problem costs more than an extra flag.
- **The flag and its wakes commit together.** An agent-driven chat's agent is woken to settle it; a user-driven one waits for its user. A pending result goes before a wake about the same run, and a flag on an ended run still wakes, to be weighed against the result.
- **A flag's state is read back, never stored,** from the choices in the run's journal and the turns that carried it. A review that ends without a choice wakes the agent once more; after that the flag is kept or left to the user, with the reason in the journal.
- **One choice wins.** `orx exp flag` and the dashboard send one choice under a request id that answers a repeat as the first. Stop goes through the run's cancel, so of a Stop, a Keep and the agent's own cancel exactly one wins.
