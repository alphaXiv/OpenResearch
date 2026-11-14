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

### The price of a run

**A run is priced once, when its job exists.** The engine asks the backend what it charges per minute or hour (`Backend::quote`) and stores the answer on the run; a later price change never rewrites it. An OpenResearch box keeps the price openresearch.sh gave when creating it. A read that fails is retried a few times, then leaves the run unpriced; it never holds up the launch. Spend is the quote times the run's running time, so a running run's grows and is an estimate. Hugging Face Jobs, Modal and OpenResearch boxes are priced, and a run of theirs without a price shows that its cost is unavailable. Vercel and Tinker have no per-run price orx can read, and the user's own machines and clusters cost nothing orx can see; their runs show no cost.

## The engine

**One engine per data directory, inside `orx up`.** The engine lock keeps it single ([storage](storage.md)). The dashboard, the desktop window, the CLI and agents are clients; none of them writes run state.

| Part | Role | Home |
| --- | --- | --- |
| Reconciler | Sole writer of run state; a fact and its notices commit in one transaction | `src/engine/reconcile.rs`, `src/engine/submit.rs` |
| Observers | One loop per host root, API account and rental, and one for this machine | `src/engine/runtime.rs` |
| Outbox | Delivers notices as hidden chat turns | `src/local/chat/` |
| Host link | One `ssh` per (host, root) to `orx-host serve` ([remote](remote.md)) | `src/engine/transport/` |

**The process that runs a job is never the engine.** A wrapper owns its payload and records the outcome in receipts on its machine; hosted services and rentals hold that state themselves. `orx up` may restart, sleep or update; it reconnects, reads what the job left and catches up. A headless `orx up` exits after a sustained idle period: no unfinished run, undelivered notice, active chat turn or client ([desktop](desktop.md) for the window).

A local run keeps its machine from idle sleep until its exit receipt, as a playing video does; closing the lid still sleeps it. Its wrapper holds the operating system's idle-sleep prevention, which dies with the wrapper. Windows runs have no wrapper, so there `orx up` holds one while any local run is unfinished.

[FROZEN.md](../crates/orx-host-proto/FROZEN.md) is the contract for host disk, the payload's environment, stored run state, API v2 and SSE, for the life of every queued job and lingering cleanup. Read it before changing any of them.

### Phase and contact

A run has two independent fields.

- **Phase** is the lifecycle: submitting, starting, running, done, failed, cancelled or lost.
- **Contact** is how fresh that knowledge is: live, unreachable, needs auth (SSH Connect or provider credentials), unreconciled (an outcome the engine cannot establish) or stopping (a cancelled run with processes left).

A run that has not started also keeps the backend's latest word on why it waits, such as a Slurm pending reason or a pod no node can fit. It is provider text, shown beside the status, written only when it changes and dropped once the run runs or ends.

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
- reply lost → recover: found → starting; absent → relaunch once, then back off with the error shown; unknown → unreconciled, retried with backoff; the backend unreachable → stay submitting, with that contact

An accepted cancel or Abandon cuts the backoff short: recovery decides at once, with the cancel in hand.

**A launch is idempotent in the run ID.** On a host, the first launch intent is published exclusively; an identical retry adopts it, a different one conflicts, and the wrapper refuses a run that already started, so a payload never runs twice. API backends find a lost launch by the run ID where the provider allows it; elsewhere the run stays unreconciled until the user abandons it.

### Cancel and Abandon

Cancelling records the intent and reason on the run. Before the fence it is immediate; after it, the backend's stop proof decides the result, and a death seen while that verdict is pending does not end the run. After an accepted cancel, a job proven gone ends cancelled, and so does a nonzero exit or signal on a hosted service, keeping the reported exit code.

The engine delivers the stop once per connection, whenever the machine is reachable, and again whenever the run loses contact; the run's wrapper performs it, so verification never races its own exit ([remote](remote.md)). A run completes when its direct child exits; processes left behind are listed, raise one notice and stop only on request.

| State | Meaning |
| --- | --- |
| cancelling, while submitting, unreachable or needs auth | The cancel has not reached the machine. |
| cancelling, with a cancel error | The backend refuses the stop; it is retried every round and shown with its error. |
| cancelling, otherwise | The stop is running. |
| cancelled, live | The stop finished and a complete scan found nothing left. |
| cancelled, stopping | Survivors outlived the stop; the wrapper keeps killing them. |
| cancelled, live, `cancelUnconfirmed` set | The stop finished, but some processes could not be checked. |

`ui/src/runDisplay.ts` builds the dashboard's labels from these fields.

The first accepted cancel keeps who asked for it beside its reason, in the same
statement: the user (the dashboard, or `orx exp cancel` outside a chat), the
chat's agent (`orx exp cancel` from its chat, with its `--reason`), or a rule.
A later stop, of any kind, never replaces it, and a cancel from before this was
kept has no one recorded, never one guessed from its words. A cancelled run's
`outcome` says who.

**Abandon** ends an unfinished run with no live contact, or whose stop its backend keeps refusing, as lost and sends the terminal notice. The kill stays pending: a terminal run without live contact is a **tombstone**, still watched, whose stop is delivered whenever the job or host returns, and which settles on the next proof that nothing is left.

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

## Run signals

Payloads may print `::orx:: {"step":1,"of":5000,"t":1.5,"m":{"loss":2.31}}`.
Progress is advisory: no measurement finishes a run or proves containment.
The vendored `orx_run.py` helper is available through `orx skill compute/assets/orx_run.py`.
Metrics live under `m`; `spend` is cumulative paid-service USD, separate from rental cost.
Declared progress wins over detected tqdm bars, epoch starts and counters that open a line,
alone, in brackets or after a noun runs count (`chunk 3/10 done`).
Detection requires two compatible reports; ordinary fractions in prose are ignored.
One detected bar is the run's progress while it is below its total. Once it is
full and the run goes on, or a bar restarts and the run has several, how much of
the run is done is unknown, though each bar still shows its step of total.

The same bounded fold runs beside native host logs and over API spools on the
workspace machine. Its checkpoint commits unfinished frame bytes, byte cursor,
latest values and bounded history together. A restart resumes that checkpoint;
a replaced or shortened log starts a new generation. A spool cut back to where
its provider continues is the exception: its checkpoint stands while the bytes
past that point are fetched again, which resume it if they return the same and
are another log if not. Corrupt checkpoints are
retained with their path in diagnostics and rebuilt from readable raw bytes.
Replayed bytes without payload or bar times do not invent timing observations.
The first phase starts when the payload started, as the host recorded it, even
on a first scan. Payload `t` gives only durations, within a stretch where it
does not fall: a relaunched process starts a new one, and its first report's
step is taken as is, so a resume from an older checkpoint moves progress back
and redraws the steps it redoes. Reports are placed in
time by the last advance, which was written no later than the log last changed:
a scan that reaches the end of the log places it at the earlier of that and the
scan's time, so a run that hung during an outage or before anyone looked shows
its silence. Every time in a checkpoint is on the clock that folded it. Each
answer from a host carries the host's clock; a successful collection stores it
with the laptop's time of receipt, and that pair maps the laptop's clock onto
the host's, so clock skew changes nothing and a silent run's clock stays
current. A fold the laptop continues from a host's checkpoint writes host times
through the same pair.

Finished runs owe signal collection independently of log collection. The end
of a run is one decision over what is persisted: the host's checkpoint, the
log the laptop kept, and whether the host can still be asked. Before release
the host gets one time-bounded request to fold to the end of its log; the
release then happens once, whatever that did. While the host still has the
run, its checkpoint is the answer and failures retry with the log debt's
backoff, telling the chat once on the seventh. Once it cannot be asked
(released, deleted or cleaned up), the laptop continues the host's checkpoint
over the bytes it kept, a pass at a time to a deadline so other runs keep their
turn, resumed across restarts, then settles: complete, or incomplete with one
notice naming the most specific cause, such as how the host went over an
earlier failure to reach it. A checkpoint is never replaced by one that knows
less: a kept log that does not reach its cursor leaves it standing, settled
incomplete. A log folded again from its start (a spool started over or
refetched with other bytes, a replaced log read through byte pages) is a rebuild, staged beside the
published checkpoint and resumed across passes and restarts; it replaces that
checkpoint once it covers the same bytes, and if its log ends first the
published one stands, incomplete. A checkpoint that cannot be read is unknown, never absent, and so
is a kept file that is gone: each settles incomplete, naming the file. A run that
never launched, has not started or reported no signals has nothing to keep: it
settles quietly and status prints no signal error for it. Status says which of
these a run is in.

Live collection follows lifecycle updates under a shared tick budget; a run the
budget does not reach keeps its status and goes first on the next tick. A paged
transfer keeps its identity across ticks.
Status labels delayed collection, the laptop's last successful collection time,
and the host data time separately. A completed bar never replaces a lifecycle
receipt.

### The monitor

A run's monitor is one projection (`src/engine/monitor.rs`) of its checkpoint,
the estimator and its lifecycle: the summary every `Run` carries, the whole
monitor `GET /api/runs/:id/monitor` serves, and the text `orx exp status`
prints, all at one `rev`. The estimate is evaluated once on the server, from
the collection's clock pair, and stamped with when; pages only count down.
A run with no total fills against its time limit, and so does one that drew
several progress bars, since how many are still to come is unknown: its bar is
one segment, never a row of finished ones. The estimate is left out only when
how much of the run is left is unknown, and is pending only while a reported
total waits on its pace, which the summary says, so a line and a strip word it
alike. Time left from a bar or counter read from the log is a floor for the
run, with no range or finishing time, since more work may follow it; only a
total the run declared promises when it ends. A run that has reported no progress, as most never will, counts down to
its time limit or says it has reported none. Cost comes only from what a
run records: its declared spend, and nothing invented for a machine without a
known rate. Without a view of its own, a run is drawn from what it reported: the
first measure it printed on its own heads it and is charted over step, then a
table for each table it reported, then its other measures; a row's columns stay
in their table. A run that reports only rows leads with its first row's table.
The order is kept in the checkpoint, so a restart draws the same.

A view of its own is markup the chat's agent writes on an experiment with
`orx exp view set` (`src/engine/view.rs`): `signal` declarations that name,
label and round what the run reports, then one `view` of blocks. The
experiment stores the text as written and every read derives the tree from it,
so there is one copy; a child experiment starts with a copy of its parent's,
as it does of the run command, and a later edit to the parent leaves it be.
Saving checks every name the view draws against what the experiment's runs
reported: a name not reported yet is pending, since it may come in a later
stage, but once runs report, a name they never did is refused with the names
they did. A view changes what is shown, never what runs: saving one draws the
experiment's runs again, and an open page starts over from it. A saved view
this orx cannot read draws the default and `orx exp status` says why.

A log an agent cannot change is read with patterns saved beside the view
(`orx exp view set --pattern`): each line the first matching regex fits
becomes a record, as if the run had printed it, with fixed or captured
step, total, stage, unit and table row, and `name`/`value` groups naming a
signal. The fold applies them where it runs, on the host or this machine,
from a configuration file beside the run's checkpoint that orx keeps in step
with the experiment and sends to a host at its next collection. New patterns
fold each run's log again from its start into a new generation, which
replaces the old one only once it has read as far: a live run at its next
collection, an ended one from the log kept here. A counter read by a pattern
promises only itself, as a detected one does. `orx exp view check` folds the
run's log as it reads now with the patterns and says what each one matched.

A run compares with its parent experiment's latest run that finished with
signals, else its newest with signals, for each signal whose view declares
which way is better: at the same stage and step, interpolated only between
steps both reported, and, while its stage has no steps, against what an
ended parent reported last; a table's column is no result to compare.
Anything else is no comparison, never a level one. A parent's checkpoint is read once and held; a live one is taken from its
own projection.

Time the laptop did not watch is unknown. A live run whose last collection is
older than collection's own once-a-minute heartbeat reads as stale, and its
estimate and stall stay where that collection left them until one succeeds.
A rebuilding checkpoint reads as rebuilding, never as lost contact.

`rev` and `seriesRev` only grow, across restarts too: `run_monitor_state` keeps
a pointer to what was last published and the summary it numbered, never the
monitor. Only `orx up` publishes, from one task that projects live runs each
second, at once on a lifecycle change, and every ten seconds for what time alone
moves. A checkpoint the laptop publishes is handed to the projection in memory,
so the hot path never reads one back, and a status rewrite that changes nothing
the projection reads projects nothing. Readers never wait on a projection under
way: they get the last one published. An ended run is listed from its stored
summary and its checkpoint is read only when it is opened; the cache holds live
runs and a few ended ones. An event carries the points and rows a run gained on
the basis it names, never a whole table. `orx exp status` reads the running
server's monitor and, with none running, projects the same from disk without
publishing; both write time as the page does.

A projection that fails, because the store refused a read or a write or the
projection broke, is said to every reader until one succeeds: the summary loses
its time left, takes its tone from the lifecycle and carries why, the run stays
watched whether or not it has ended, and a read tries again. A `run.monitor`
reset and a `run.updated` go out when it starts failing and when it recovers.
The log says the cause when it starts, when it changes and when it ends.

### Rules

An agent declares rules beside an experiment's stage plan (`orx exp view set
--config`, `src/engine/rules.rs`): each names one bounded condition on what a run
reports, never an expression, and stops the run or raises a flag. A child
experiment starts with a copy, and a run freezes the rules it was launched with,
so editing them changes only later runs; `--budget` adds one. Only `orx up`
checks them, in the projection's pass, and only on a running run whose contact
and data are live and whose stop is not already under way: what it cannot see
stops nothing. Each pass counts the reports a run made since the last, by where
they fall on the signal's axis, and a rule acts only if its condition holds once
they are counted, so a value that already recovered, as in history read after a
sleep, stops nothing. What each rule counted, and that it acted, is kept with
the run's monitor state in the transaction that records the event, the flag
an `ask` raises and a `stop`'s cancel, so a restart neither counts a report twice
nor acts twice. A rule on the estimate (cost or the time limit) acts only on a
pace measured over four slices and seen on two revisions; spend already past a
budget stops at once. A stop goes through the engine's cancel, the first stop
wins, and the run is Auto-stopped by that rule, with no model involved.

A stopped run reads **Revising** while the chat's agent repairs its experiment:
`orx exp revise EXP --run RUN`, from that agent's chat during its turn, records
the turn against the experiment's newest run, if that run was stopped. The
server derives the state, never stores it: it holds while that turn is going
and no run of the experiment has started since, and ends by itself when either
changes. The monitor task keeps such a run watched, so its line hears at once.
Nothing claims the repair happened; the next run is the proof.

### The watcher

One cheap model per chat watches the chat's runs and writes each run's sentence
(`src/engine/watch.rs`). Which model is one function's answer: Claude Code's
Haiku one-shot on Claude Code, the GPT Luna Codex's own catalog offers on Codex,
and on any other agent none until the user picks one. A look is a one-shot with
no tools: Claude Code's are switched off by flag, Codex's by its feature switches
with a turn that starts one anyway stopped and its answer refused. A harness that
cannot promise that is not used.

A run's owner is the chat that last looked at it, else the first chat following
it that has a watching model. One look covers every live run the chat owns, at
most six a call and four calls a slot. It is due when something changes: a
number the sentence binds leaves its trend, a comparison with the parent changes
sign, a counted pattern moves, a stage starts or ends, the stage runs at least
twice the parent's pace, or the log prints a kind of warning it has not printed
before. Between changes, a ladder from each stage's start (1, 2, 5, 10, 20
minutes, then the experiment's `maxGapS`) bounds the silence while the run keeps
printing; with nothing new there is no look. Looks a change brings keep the
chat's `minGapS` between them and never outnumber the ladder's points since the
oldest run started. A look answered `same` doubles the threshold of the check
that brought it. A run gets one last look when it ends, shared with siblings
ending within a minute.

The digest is bounded at 16 KiB: the brief, each run's progress, signals and
last three sentences, the log's head and its new lines, new kinds of warning
first, repeats said once with their count. Each field and log line passes
sharing's credential scan as written, before it is clipped or encoded; one that
matches is left out, saying so, and the journal tells the user once where it is.
A retry's reply is scanned before it goes back. A reply is strict JSON: each number that can
move is a token orx fills, exactly one figure is bold, a sentence reads at most
90 characters filled, and a number written as the value it reads now is refused.
One retry carries what was wrong; a run whose answer stays wrong keeps its last
sentence. Each look is a row written before it is sent and settled in one
transaction; a look `orx up` stopped after sending is recorded as interrupted and
never sent again, so evidence is never lost and a model is never billed twice
for one look. A look that flags raises a flag in the transaction that settles it.

### Flags

A flag says a run may be going wrong and never stops it (`src/engine/flags.rs`).
Three things raise one: a look, a rule that asks, and orx's own tripwires, which
watch every live run whether or not a model does (`src/engine/watch/trip.rs`): a
value that is not a number, unless a rule the run was launched with checks that
signal for it; no progress past the estimator's stall threshold, saying whether
the log went silent too; a run that reports no progress and prints nothing for
ten minutes; a kind of error line the run has not printed before. A flag keeps
the log lines it cites, by bytes, line numbers and the log's identity, and the
look that raised it groups the flags of runs it saw together.

A run has at most one open flag from a look or a tripwire, so a trouble that
lasts wakes once; a tripwire condition is raised once a run, and never again on
an experiment where someone kept going after it. The flag and the notices that
wake each following Agentic chat are written in one transaction; a Copilot chat
is woken only when its user asks. A wake says the watcher saw only a digest and
none of the chat, cites the lines and how to settle it, and is frozen when it is
written; a chat past 70% of its context is compacted, in the turn slot the wake
already holds, before it is sent. A wake not yet claimed when its run ends is
withdrawn, since the run's end is its own notice; a flag raised on an ended run
still wakes, to be weighed against the result.

What became of a flag is read back, never stored: its choices in the run's
journal and the notice and turn that carried it give its state. An Agentic flag
is raised, then waking, then the agent is looking; a turn that ends without
settling it, an agent's `--ask` and a Copilot flag wait for the user, who sees
Stop run, Keep going and Ask in the strip and an amber mark on the chat's row.
`orx exp flag RUN --flag ID --keep|--ask|--stop` and the strip send one choice;
a repeated request id answers as the first did, and a settled flag, or one on a
run that ended, is refused. Keep going settles it and the run's next looks read
what was kept and why. Stop cancels the run as `orx exp cancel` does. Ask sends
the flag, and every open flag its look raised beside it, to the chat's agent as
one turn.
