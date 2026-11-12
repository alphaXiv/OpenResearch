# The dashboard

`orx up` is the long-lived web server behind the dashboard, with a React app in front of it. The app's API has no accounts and no login, so its safety rests on who can reach it. The server is also one writer among many: CLI commands, agents and supervisors change the same store while it runs, and the page has to show their changes as they happen.

## What `orx up` hosts

`orx up` (`src/commands/up.rs`, `src/commands/up/`) binds `127.0.0.1` on a fixed default port, one server per data directory ([storage](storage.md)). It serves the built app, a JSON API under `/api` and an event stream, and owns the work that needs a resident process:

- the chat host: agents, turns, queues, pending approvals ([agents](agents.md));
- terminals, each a PTY over a websocket;
- file serving for the editor, previews and artifacts;
- the run engine ([experiments](experiments.md));
- remote workspace gateways ([remote](remote.md));
- background passes: agent detection, model catalogs, cleanup sweeps, indexing older chats.

The same server runs inside the desktop app, headless for the CLI, and as a persistent remote host.

**It does not host the jobs.** Each run's wrapper is its own process, so a dashboard can stop or restart without losing a job ([experiments](experiments.md)).

## The loopback guard

Loopback keeps other machines out, but not pages open in the user's browser. **Every request passes one guard** (`src/commands/up_remote.rs`):

- **Host** must name a loopback address or `localhost`, which defeats DNS rebinding.
- **Any method but GET, HEAD or OPTIONS, and any websocket,** is refused when `Sec-Fetch-Site` says `cross-site`, and its `Origin`, when present, must be the server's own. A websocket must send an `Origin`.
- **Responses** forbid MIME sniffing and referrers. HTML also forbids framing; other types stay frameable for same-origin file previews.

So GET handlers must not change state. No route sends CORS headers, so a cross-site page can fire a read but never see the answer. A request without an `Origin` is a non-browser client on the same machine, which already has the user's privileges.

`orx up --host 0.0.0.0` opts into a trusted private network, such as a headless workstation driven from a laptop; anyone on that network can use the dashboard. orx's own clients still use `127.0.0.1`. The guard then also refuses any peer outside the loopback, RFC 1918, link-local and CGNAT IPv4 ranges, and accepts a Host only when it is `localhost` or a literal address in those ranges, so DNS rebinding still fails.

Signing in to alphaXiv runs in the server, which tells the page where to approve it; signing in to OpenResearch runs `orx login` in a terminal the page reads. A page that reached the server by another address than loopback, or a server inside SSH, gets alphaXiv's link and code for the device grant ([remote](remote.md)) rather than a redirect its browser cannot follow; the page opens only that link, in a tab it reserved during the click so pop-up blockers allow it.

The only extra origin accepted is the dev UI server's, `ORX_UI_DEV_ORIGIN`, which must be loopback HTTP; a remote workspace's own gateway port refuses it. Terminal websockets re-check same-origin in the handler with no allowance.

Credentials stack on top: `/api/internal/` needs a token issued to that agent ([agents](agents.md)), `/api/v2/` the server's bearer from its owner-only descriptor, apart from the hello challenge ([experiments](experiments.md)), and every other path on a persistent remote host a live attachment's bearer, apart from its own agents' run callbacks ([remote](remote.md)).

**Project files are untrusted content on the dashboard's origin.** `src/commands/file_serve.rs` gives SVG, HTML and XML a sandboxing CSP, so agent-written markup cannot script the API; new routes serving project bytes go through it.

## API conventions

Errors are JSON with a message the UI can show. The store is synchronous SQLite, so handlers move file walks, Git, archive building and other slow work onto blocking threads. Work a CLI command could race is coordinated through leases and claims in the store ([storage](storage.md)), because an in-memory lock stops at the process boundary.

**No request waits on the network for long.** WebKit gives up on a response after a minute of silence, and a closed request must not cancel half-done work. Opening a shared project and publishing one run as operations: with `background=true` the request answers at once with an `operationId`, and the page polls `GET /api/operations/{id}` for the phase, bytes moved, and the result or error. A finished operation stays readable for a few minutes, so a lost poll can ask again, and the work itself gives up on a connection that stops moving. Operations live in memory, so after a restart the poll's 404 tells the user to try again. Without `background`, an older page still waits for the result.

## Live state

The visible workspace opens one event stream, `/api/events`, which is also the page's only evidence that the server is up (`ui/src/events.ts`). Three sources feed it:

| Source | Carries | If a subscriber falls behind |
| --- | --- | --- |
| per-subscriber diff of the store and run logs, on a short interval | projects, experiments, artifact pings, log bytes, update status | a small channel backpressures the loop; log bytes per tick are capped |
| engine event bus | run and host changes, engine log growth | re-reads recent runs from the store |
| chat host broadcast | chat state, harness sign-in and catalogs, data-directory moves, Overleaf sync | the server sends `resync.required` |

**Only the diff sees other processes' writes**, so polling the store stays necessary. Events patch queries already loaded; the baseline comes from the queries themselves, and chat events are edge-only, with no replay on connect.

**A gap is repaired by refetching.** Reopening a stream, after a drop or a hidden workspace, and `resync.required` both invalidate the live query families (`ui/src/queries/invalidation.ts`). A query that renders live data belongs in that list.

## Serving the app

A release binary embeds the built app, so the binary is the whole product; a release build fails if the app was not built first.

In development, Vite serves the app and proxies to a backend (`ui/vite.config.ts`), presenting same-origin browser requests as the backend's own origin so they pass the guard and the terminal check.

## The app

The dashboard is a React app with file-based routes (TanStack Router, `ui/src/routes/`) and **TanStack Query as the single store of server state** (`ui/src/queries/`). Components read through queries and write through the workspace's API (`ui/src/api.ts`). Only the log viewer's window (`ui/src/logWindow.ts`) and layout's write-behind copy hold server data outside it.

- **Events update the cache.** Entity events are written into matching queries, keeping the newer `updatedAt`, and a read that overlapped a live event is repaired instead of overwriting it (`ui/src/queries/live.ts`). Other events invalidate query families. A query keyed by a commit hash is immutable and never invalidated.
- **Optimistic writes are defined once** (`@clo/react-mutation`; the client in `ui/src/queries/mutations.ts`, each definition beside the queries it changes): a definition takes the workspace it writes to, applies its change to the cache, and rolls it back and refetches if the server refuses. A chat's settings writes and sends run in order through `workspace.chat.serialize`.
- **Writes invalidate by URL.** A successful write invalidates the families mapped to its path in `ui/src/queries/invalidation.ts`; a new write route that changes rendered data needs an entry there or an event.
- **A workspace is one machine** (`ui/src/workspace.tsx`, `ui/src/Machines.tsx`): its own immutable API base path, lifetime-scoped keys and memory-history router. Every machine shares one QueryClient, so a view across machines reads one cache; keys never omit their workspace scope. A remote machine's requests go through `/_orx/workspaces/<session>/` on the same origin ([remote](remote.md)). The visible machine mirrors its location to the browser URL, with `?machine=<session>` naming a remote.
- **A hidden machine** keeps its cache, route and drafts and closes its stream, so returning paints immediately and then repairs. Reads, writes, streams and resource URLs stay bound to the machine that made them. Replacing the data directory, or a remote session resolving to a different database, disposes the lifetime and aborts its requests.
- **Layout is server state too.** Per-project tabs and the app's last location and panels live in the store (`ui/src/workspaceState.ts`, `src/workspace_state.rs`), saved in the background and flushed when the page hides, so the browser and the desktop app reopen where the user left off. Resuming checks that the location still exists (`ui/src/routeResume.ts`).
- **Chat search** reads the index on the workspace's own machine only while open, and pages results ([storage](storage.md)).

The desktop app's window adds what only it can do ([desktop](desktop.md)).

## Design tokens and text

`ui/scripts/check-styles.mjs` and `ui/scripts/check-i18n.mjs` enforce the token and catalog rules in `AGENTS.md`; every locale carries the base locale's keys with the same placeholders. No lint covers interpolation: call sites wrap values with the isolate helpers in `ui/src/i18n.ts`, so a name or number cannot reorder the sentence around it.

## Restarting into an update

**A server relaunches into a new binary only when that interrupts nothing** (`src/commands/up.rs`). Without a restart, a long-lived `orx up` keeps launching runs on old code.

A terminal or headless `orx up` with automatic updates tries by itself once a deferred update is due or a newer binary is on disk ([desktop](desktop.md)). The desktop app's server restarts only when the page asks through `POST /api/update/restart`, and a busy server refuses. A persistent remote host does neither.

Blocking work is any API request whose response is still being sent, open terminal, data-directory move, remote session not disconnected or awaiting an install, and chat turn running, queued or waiting on the user. Event streams and the CLI's `hello` neither count nor wait, because a page holds its stream for as long as it is open.

1. Stop admitting new requests; they wait.
2. If blocking work remains, reopen admission and try again later.
3. Commit. Admission never reopens and no new turn starts, so nothing begins on old code that a new process would have to adopt.
4. Stop agents and gateways, release the install lock, and install any deferred update. A failed install relaunches on the current version and backs off instead of looping.
5. Relaunch. The page polls update status until `instance` changes, then reloads.
