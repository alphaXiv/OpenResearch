# Failure-first testing

Write the realistic failure scenarios before implementing a change. Prefer a
workflow through the dashboard, real Rust API, and isolated SQLite store.
Use controlled fixtures at external boundaries, never a replacement React hook
runtime or snippets extracted from component source. For a regression, first
demonstrate that the scenario fails on the broken behavior.

## Failure scenarios for this migration

| Workflow | Failures to exercise before removing existing coverage |
| --- | --- |
| Model selection and sending | A manually entered model is replaced by a catalog refresh; an empty catalog prevents sending; CLI default still sends the previous model; model-specific reasoning/speed are not reconciled; the selection displayed in the browser differs from the persisted selection or native request. |
| Workspace connections | The home/sidebar connection control is missing; configure/close/Escape loses the host picker; loading, error, onboarding, or SSH runtime hides a required control; popup blocking sends a connection request anyway; failure leaves the placeholder tab open; success navigates the wrong tab. |
| Compute settings | Tinker is missing from compute/environment settings or run labels; a refreshed setting overwrites a dirty input; a clean input ignores a refresh; a failed refresh erases previously loaded rows. |
| Selected run | An omitted run does not choose the newest matching run; an explicit old run is replaced by the newest; an unknown or foreign run displays another run's log. |
| Transcript | Interacted rows disappear outside the viewport; full-history mode omits messages; updated tool labels remain stale; resized content moves a reader away from their position; switching chats preserves the previous chat's bottom-pin state. |
| File restoration | Opening a saved workspace triggers compile/sync without consent; explicit compile/sync runs twice; credentials alone activate sync; ordinary file opening stops activating compile/sync. |
| Chat and workspace concurrency | Sending before cold history arrives loses/duplicates a turn; delayed responses update the wrong workspace; cached rows disappear after refresh failure; a newer draft is cleared by an older send. |

Remove an existing check only when the replacement asserts the same observable
failure. Keep isolated tests for security boundaries, exact event ordering,
protocol parsing, and external integrations that cannot be exercised reliably
in the local workflow. Record the failure list before modifying those tests.

Do not count format, Clippy, typechecking, or source-text matching as behavior
coverage. Do not delete a test just because it is small or because a happy-path
workflow touches the same function.

## Running and inspecting evidence

On macOS or Linux, install Node 22+, pnpm 10, Rust, Git, SQLite and `lsof`.
In a worktree (with a registered `main` checkout), run:

```sh
pnpm -C ui install --frozen-lockfile
pnpm -C ui exec playwright install chromium
pnpm -C ui test:e2e
pnpm -C ui exec playwright show-report e2e-report
```

Every run starts an empty dev slot, uses an isolated HOME and a deterministic
Codex JSON-RPC executable, then stops the slot before saving the SQLite data.
No real agent credentials or SSH/Overleaf/Tinker account are needed. The paper
and remote-connection contracts replace external HTTP responses; the dashboard
runs its actual React hooks. Home-state contracts control gateway snapshots and
the event stream so local project events cannot overwrite the injected state.
Compute-refresh contracts control the settings API and advance browser time.
They do not contact a cluster.

`ui/e2e-report/` contains the HTML report; `ui/e2e-results/` contains replayable
Playwright traces, screenshots, videos, native agent requests, saved files,
backend/UI logs, the stopped data snapshot and the revision/dirty-tree marker.
Rerunning the command creates fresh fixtures; IDs and timestamps can differ.
CI runs this suite inside the required `fmt, clippy, test` job and uploads both
directories as `dashboard-e2e`, including when a test fails. CI keeps the default
PR merge checkout; its extra main worktree exists only for dev-slot bookkeeping.

## Suite audit and migration boundary

The Rust suite includes checks using actual SQLite, files, subprocesses,
backend protocols and failure recovery. Keep their security, migration, parser,
retry and concurrency cases. Changing their count without replacing those
failure modes would reduce protection. The existing platform/packaging checks,
ignored production telemetry contract, dev-slot and release-note checks remain.

The browser workflows replace the fake-hook/source tests for model selection,
run selection, workspace connections, passive file restore and Tinker visibility.
Compute clean/dirty refresh cases move out of `queryConsumers.test.mjs`.
Full-history, resize and chat-switch cases move out of
`transcriptVirtualization.test.mjs`; its immutable-part cache and deleted-branch
range checks remain isolated because they assert exact cache-call and index
behavior. The cold-history/workspace-retirement race checks and cached-refresh
error checks remain until the same failure ordering has browser coverage.

Agent playbook/policy contracts remain as checks of instructions passed to
external tools.

Other UI checks exercise command parsing, annotations, markdown/file trust
boundaries, transcript branching, workspace serialization and persistence.
Keep them; favor a failure-first workflow when extending those features.
