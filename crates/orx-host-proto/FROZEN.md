# Frozen run-engine surfaces

This is the canonical compatibility contract for the first run-engine release.
It covers the full lifetime of queued jobs, payloads and lingering cleanup;
there is no time-based compatibility cutoff. Shipped 0.2.x supervisors and
Windows local runs retain their existing formats. Draft engine formats have
no migration or backfill obligation.

## Evolution

There is no per-run `v` field. Presence-authority files are never renamed:
`launch.json`, `wrapper.json`, `started`, `cancel.req`, `cleanup.req`,
`job.sbatch`, `slurm.job`. They may only gain optional fields. A missing new
name must never let an older reader infer that an existing execution is absent.
Required proof fields remain required; `#[serde(default)]` is only for optional
additions. Unknown object fields are tolerated; unknown variants and malformed
JSON are uncertainty. Read bytes before parsing so I/O errors retain their type.

Any breaking host-disk change also raises the database `min_reader_version`.
Every launching laptop checks that floor before migrations, which fences its
run-directory readers after a downgrade. New binaries cannot fence an already
running or shipped 0.2.x writer: database changes remain additive while those
writers may exist. A new receipt filename can introduce a breaking receipt
shape only alongside that floor; it cannot replace a presence-authority file.

The JSON files in `tests/fixtures/` are append-only. Historical fixtures must
remain readable and today's writers must match an approved fixture. Add a
fixture for an intentional extension; never rewrite an old fixture to hide an
incompatible writer. After release, also exercise the actual shipped reader
and wrapper. A new control cannot require an old wrapper to understand it.

## Host disk and execution

| Surface | Contract |
|---|---|
| Private root | `bin/`, `runs/<run>/`, `sources/`; run IDs are 1–200 ASCII alphanumeric, `-` or `_` bytes. Source IDs are lowercase SHA-256 of the archive bytes. One run directory permits at most one payload start. |
| `launch.json` | `LaunchIntent`: `run`, `digest`, `run_sh`, `env`, optional `source` and `cancel_sh`. Identity is `(run, digest)`; digest includes source identity. The first accepted intent governs scripts, environment, source, container ID/generation and Slurm directives. Retries adopt those bytes. |
| `job.sbatch`, `slurm.job` | The batch script is both Slurm ownership and a pinned executable reference; the job ID is numeric text. Direct/Slurm ownership is checked under `launch.lock`. A batch-only preparation interrupted before `launch.json` can resume its first submission; sbatch only runs after that identity is published. Execution evidence (`slurm.job`, `wrapper.json`, `started`, `exit.json`) without a readable launch key is uncertainty, never permission to submit a replacement. |
| Slurm submission | Job name is the run ID; scheduler output is `slurm.out` in the run directory. Submit with `--no-requeue`. PREEMPTED/NODE_FAIL without a receipt become Failed after receipt grace; a forced requeue cannot rerun the payload and may end Lost. No automatic attempts. |
| `slurm-terminal.json` | Optional per-run retained scheduler evidence: required `job`, `state`, `since` (SystemTime object with `secs_since_epoch` and `nanos_since_epoch`). Receipt grace starts at `since` and survives serve replacement and unavailable accounting. A subsequent live scheduler state clears it. It confers no launch or deletion authority; future fields must be optional. |
| `wrapper.json` | Current attempt identity: required `node`, `boot_id`, `pid`, `starttime`. Linux node identity is hostname; macOS is `gethostuuid()` and errors fail closed. Linux starttime is `/proc/<pid>/stat` field 22 in clock ticks since boot; macOS is process start time in microseconds since epoch. Local scans, liveness and signals require the recorded node and boot. Same-node reboot proves loss; another node proves nothing about local PIDs. |
| `started` | Exclusive, fsynced text `<run>:<nonce>\n`. Presence forbids another start, even if a crash prevented spawn. It is an at-most-once fence, not proof the child ran. |
| Locks | `fcntl(F_SETLK)` on `launch.lock` protects launch preparation and PinSource; `wrapper.lock` is held for the wrapper lifetime; `start.lock` orders cancel/control publication against the last pre-spawn check and `started`. A process-local run mutex also serializes mutators because fcntl locks are per process. Shared filesystems must honor cross-node locks. Wrapper-ownership probes are nonblocking. |
| Publication | Write temporary file, fsync, rename (exclusive hard link for initial intent), then fsync parent. Readers see whole receipts. Scripts/staging resume under launch ownership; never rewrite accepted bytes from a retry. |
| `exit.json` | Immutable final run receipt: optional `code`, `signal` and `finished_at` (wall-clock milliseconds at completion), required `reason`, `processes`, `outcome`. Reason spellings: `exited`, `cancelled`, `spawn_failed`. Direct-child exit is the result even when descendants remain. Cancel before start has no code/signal and verified-empty proof; spawn failure has `spawn_failed`. |
| Proof | `Survivor` requires `pid`, `starttime`, `state`. Outcome spellings are `verified`, `survivors`, `unverifiable` (required `unreadable` list). Empty processes alone is not Verified. Native proof is limited to the scanned process/tag domain. A container hook never verifies all in-container processes. Slurm step teardown owns allocation containment, so the wrapper exits after its receipt, including a cancelled receipt with survivors. |
| `cleanup.json` | Latest `ProcessOutcome {processes,outcome}` renewal. After wrapper death on the recorded node, serve renews proof by a fresh tag/known-identity scan and kill pass; a changed boot verifies emptiness. It supersedes containment evidence, never the result in `exit.json`. After cancelling with survivors the wrapper keeps scanning and killing on a 1–30 s backoff, without re-running the hook or TERM grace. A prior incomplete scan or hook cannot become Verified merely because a fixed PID list emptied. |
| Controls | `cancel.req` / `cleanup.req` are presence markers, fsynced with their directory under `start.lock`. SIGUSR1 only wakes polling; it does not itself encode cancellation. Failed cancel hooks retry with backoff before any cancelled receipt; paused containers keep cancellation pending. |
| Output/staging | Raw append bytes in `log` and `notify`; diagnostics in `wrapper.log`; accepted scripts in `run.sh` / `cancel.sh`; `source.tar`, extracted `repo/` and resumable `.part` / `.synced` files keep their meanings. Unknown optional files confer no launch/deletion authority. |
| Rental duty | Optional owner-only `rental.json` retains the shipped `{rental:{apiUrl,sandboxId,token},directory}` capability-file shape. Native wrappers use their own run directory; `directory` remains for old supervisors. Activate precedes `started`; a separate thread heartbeats every 30 s through payload completion and at most 15 min of collection grace. `final_collected` acknowledges kept output. Release retries without heartbeats; requests carry only `{}`. |
| Binaries | `bin/orx-host-<sha256>` remains runnable while a pending batch script references it or live supervision/cleanup needs it. A running wrapper keeps its code; updating `serve` never upgrades that wrapper in place. |
| Native Unix local | Local/Tinker directories use the same host intent, identity, markers, receipt and proof semantics; launch detaches without leaving the engine as parent. Windows and actual 0.2.x directories remain distinct. |
| Windows local | Preserve `windows_job`, `pid`, `pid_script`, `pid_identity` where used, `started`, digest launch file, payload/run scripts and atomic integer `exit_code`. Completion of the direct child does not imply Job Object emptiness. A missing Job Object proves no member remains. |

The wrapper locks before publishing identity; `start.lock` covers its final
cancel check through `started`. A dead wrapper never authorizes rerun. Read,
parse or inspection failure is not absence, payload failure or verified stop.
On Linux without pidfd support, starttime recheck followed by `kill` is the
accepted unpinned signalling fallback. Disconnect survival requires the host
not to kill the session through logind `KillUserProcesses=yes`.

## Payload-facing surface

`ORX_RUN_TAG` is opaque and inherited. The wrapper sets `ORX_NOTIFY_FILE` to its
append-only free-text input path, uses the run directory as cwd, null stdin,
and combines stdout/stderr into `log`. Container/provider jobs only have a
usable notify path where that backend explicitly wires it. Existing processes
retain their environment and scripts across engine updates.

Keep the run context names `ORX_RUN_ID`, `ORX_EXPERIMENT`,
`ORX_EXPERIMENT_TITLE`, `ORX_PARENT_EXPERIMENT`, `ORX_COMMIT` and
`TRACKIO_PROJECT`. Reserve the log-line prefix `::orx::` and the run-view field
`progress` for future payload progress. Progress is advisory and cannot change
lifecycle `status`, settle containment or complete a wait. No progress protocol
or authenticated payload endpoint is implemented by this reservation.

## Laptop persistence and public clients

| Surface | Contract |
|---|---|
| SQLite / stored JSON | Additive schema; phases `submitting`, `starting`, `running`, `done`, `failed`, `cancelled`, `lost`; Contact NULL means Live, otherwise serialized Unreachable/NeedsAuth/Unreconciled/Stopping. Stopping carries `since` and survivor `count`; the host is derived from the run handle or alias. Preserve Handle (`Ssh`, `Slurm`, `Local`, `Tinker`, `Api{provider, account:{target, context}, ids}`, `Legacy{descriptor}`, `OpenResearch{descriptor}`; OpenResearch owns a rented native wrapper while Legacy retains already-running rental supervisors; API ids list owned resources, primary first; `vercel_job` ids are `[sandbox_name, process_id]`), LaunchSpec/options, SubmissionFence/SubmitterIdentity, NotifyCursor, subscriptions, notices, collection obligations and queued cleanup meanings. Unknown stored data must be preserved on rewrite or rejected. |
| Tombstone | Terminal phase plus non-Live contact continues recovery/kill without changing the result or sending another terminal notice. A recovered handle attaches to the same fenced attempt. Tombstones do not block idle; latest authoritative empty proof settles contact. A cancelled receipt with survivors encodes `cancelled` + Stopping (kill pending), displayed as Not fully stopped with their count and host. An empty proof encodes `cancelled` + Live, with `cancel_unconfirmed` recording incomplete containment independently of editable result prose (null for confirmed, unreadable PIDs for an incomplete scan, an empty list for uncovered processes); the dashboard localizes its note from that list and displays “Cancelled · couldn't confirm”, while CLI and chat use English. Cancel remains pending throughout the stop window; time alone never ends a run. A cancelled tombstone settles on the latest empty process proof or authoritative scheduler teardown. Only a prior Not fully stopped terminal notice permits the deduped cleanup follow-up; it is an attention notice, never a second terminal notice. |
| Kept output | `run-logs/<id>.log`, `.tail.log`, `.ranges.json`; resumable `.part` plus collection-marker identity/offset tuples. Preserve KeptRange and FileIdent encodings, byte ranges and EOF meaning. Treat identities as opaque; mismatch resets reading, never validates different bytes. |
| Discovery | Private server descriptor: `proto:2`, instance, pid/starttime, endpoint and token; authenticated HMAC hello verifies the owner. Live but not-ready/incompatible is distinct from absent. Restart/discovery must not spawn a competitor while the recorded owner remains alive. |
| API v2 | Preserve route/method/query/action semantics and run-view field names/types (including `id`, `experimentId`, lifecycle string `status`, `durationSecs`, `updatedDisplay`, `cancelUnconfirmed` nullable unreadable-PID list (empty for uncovered processes), `waitingReason` nullable provider text shown only while submitting or starting, and derived `host`). Contact and derived cancelling stay independent of phase. Launch, cancel, abandon, cleanup, logs, data/reset pages, ident/eof and unavailable/503 retain their meanings. Optional additions cannot replace existing fields. New lifecycle meanings require deliberate API versioning. |
| SSE | `snapshot`, `run`, `error`, `resync`, `drained`; preserve payload shapes, reconnect/refetch and lag behavior. Unknown event names must be skipped by the first shipped CLI (O6), so future events cannot hang or end an old wait. SSE is a live feed, not a durable telemetry journal; dashboard `/api/events` is another consumer. |
| Agent notices | Persist frozen batch membership and `client_turn_id` dedupe/retry/retirement state. Delivered means harness acceptance, with uncertain delivery retained for recovery. Payload text is data, never instructions. Terminal collection and notice ingestion must survive a restart. |
| SSH sharing | Keep `~/.ssh/openresearch_config`, `openresearch_match`, `openresearch_connections.json` and the shared `.orx-config.lock`. First use adds the shipped `Host *` / `Include openresearch_config` footer once; never remove the user's Include. Preserve Publication's `host`, `native_id`, `socket_name`, `queries` shape and the `Match final originalhost … exec openresearch_match … %C` route/socket check. Set `ORX_SSH_CONTROL_DIR` selects that scope; unset selects the default production directory. Withdraw closed or changed-route masters; absent a matching live master, plain ssh falls back normally and cannot cause an engine login. |

O6 (unknown-SSE-name handling) and O7 (database reader floor before migrations)
are first-ship-only mechanisms: an already-shipped binary cannot acquire them
retroactively. The floor protects deliberate incompatible changes, not routine
additive migrations.

## Not frozen

`RunState`, `Summary` and `Request` are internal matching engine/serve RPC types;
Hello requires the exact protocol version and binary SHA. Notice prose,
`result_markdown` wording, retention policy and implementation details are not
frozen. The persisted/public contracts above still constrain any implementation.
