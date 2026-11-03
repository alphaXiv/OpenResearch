# The dashboard

`orx up` is the long-lived process behind the dashboard: a web server on the
user's own machine with a React app in front of it. It has no accounts and no
login, so its safety rests entirely on who can reach it. It is also one of
many writers: CLI commands, agents and supervisors change the same store
while it runs, and the page has to show their changes as they happen.

## One server on loopback

`orx up` binds `127.0.0.1` on a fixed default port and serves three things:
the built app, a JSON API under `/api`, and a server-sent event stream. While
it runs it owns the work that needs a resident process:

- the chat host: agent processes, turns, queues and pending approvals
  ([agents](agents.md));
- terminals, each a PTY relayed over a websocket;
- file serving for the editor, previews and artifacts, with byte ranges;
- background passes: agent detection and model catalogs, starting a
  supervisor for any active run that has none, sweeping remote run
  directories and orphaned files;
- remote workspace gateways ([remote](remote.md)).

Runs are not on this list. A run's supervisor is its own process
([experiments](experiments.md)), so a dashboard can stop or restart without
losing a job.

## The loopback guard

Binding to loopback keeps other machines out, but not web pages. Any site the
user visits can make the browser send requests to `127.0.0.1`. Every request
passes one guard:

- **Host** must name a loopback address or `localhost`, which defeats DNS
  rebinding.
- A request that changes state, or opens a websocket, is refused when
  `Sec-Fetch-Site` says it is cross-site, and its `Origin`, when present,
  must be the server's own. A websocket must send an `Origin`.
- Responses carry headers that forbid framing, MIME sniffing and referrers.

A request without an `Origin` is a CLI or another non-browser client on the
same machine, which already has the user's privileges. The only extra origin
the guard accepts is the development UI server's, named by an environment
variable and itself required to be a loopback HTTP origin. Tool approvals
from an agent's permission bridge also require the token that bridge was
spawned with. A persistent remote host requires a bearer token on every
request, and gives its own agent subprocesses a token of their own.

## API conventions

Errors are JSON with a message the UI can show. The store is synchronous
SQLite, so handlers move file walks, Git, archive building and other slow
work onto blocking threads and keep the async runtime responsive. Work that
another dashboard or a CLI command could race is coordinated through leases
and claims in the store, not only in-memory locks, because they may share the
database.

## Live state

```text
store + run logs ─► per-subscriber diff, every 500 ms ─┐
                                                       ├─► one SSE stream ─► page
chat host ─► broadcast as it happens ──────────────────┘
```

The page opens one event stream. Two sources feed it:

- **A diff loop over the store and the run logs.** Other processes write the
  store, so polling it is the only way to see their changes. Each subscriber
  keeps a cursor of what it has seen, and the first pass sends everything, so
  a fresh page needs no separate snapshot. Events carry whole entities:
  projects, experiments, runs, run-log bytes from the last offset, artifact
  changes, update status, data-directory move progress. Log bytes per tick
  are capped, and the channel is small, so a slow page backpressures the loop
  instead of growing a queue.
- **Broadcasts from the chat host.** Chat messages, sessions, queues and busy
  state go out the moment they change. If a subscriber falls behind and the
  broadcast drops events, the server tells the page to resync.

## Serving the app

A release binary embeds the built app, so the binary is the whole product; a
release build fails if the app was not built first. The HTML entry point is
always revalidated, and the hashed assets are cached forever. Every non-API
path falls back to the entry point for client-side routing.

In development, Vite serves the app and proxies the API and websockets to a
backend. The backend accepts the dev server's origin through the allowance
above, and the proxy presents same-origin browser requests to the backend as
coming from the backend's own origin.

## The app

The dashboard is a React app with file-based routes (TanStack Router) and
**TanStack Query as the single store of server state**. Components read
through queries and change things through mutations; nothing else caches what
the server knows.

- **Events update the cache.** Entity events are written into the matching
  queries, keeping whichever copy has the newer `updatedAt`. Other events
  invalidate families of queries. A query keyed by a commit hash is immutable
  and never invalidated.
- **Workspaces scope every key.** Each query key begins with the current
  workspace: this machine or a remote one. Switching workspaces discards the
  old scope, so data from one machine can never appear in another's view.
- **Layout is server state too.** Each project remembers its tabs per chat,
  the active pane, history, scroll and expanded folders; the app remembers
  the last location, panel sizes and the sidebar. Both are saved to the store
  in the background and flushed when the page hides, so the browser and the
  desktop app reopen where the user left off. Opening a project or the app
  resumes the last location that still exists, and falls back to the newest
  chat otherwise.

## Design system and text

Colors come from theme tokens defined for light and dark themes; the theme
follows the system unless the user picks one. A lint rejects raw colors and
arbitrary color or text-size values outside the theme, so a new component
cannot drift from the palette. All visible text goes through Paraglide
message catalogs, and a lint requires every locale to carry exactly the base
locale's keys, each non-empty. Interpolated values are wrapped in Unicode
isolates so a name or number cannot reorder the sentence around it.

## Inside the desktop app

The page detects the desktop app and adds what only it has: a native menu
whose structure the page defines and sends to the window, zoom controls,
custom window controls where the platform draws none, and navigation for
`openresearch:` links. The window's side belongs to [desktop](desktop.md).

## Restarting into an update

When an update lands on disk underneath a running `orx up`, the server
relaunches into the new binary once doing so interrupts nothing. In-flight
work is every API request whose response is still being sent, every open
terminal, a data-directory move, a connected remote workspace, and any chat
turn that is running, queued or waiting on the user. The event stream does not
count, because a page keeps it open for as long as the page is open.

To restart, the server first stops admitting new requests (they wait), then
checks that nothing is in flight. If something is, admission reopens and it
tries again later. Once it commits, admission never reopens, and no new turn
starts, so nothing begins on old code that a new process would have to adopt.
