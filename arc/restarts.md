# Surviving restarts

`orx up` restarts all the time: every update, every crash, every rebuild while developing it. A restart must not end an agent mid-turn, close a terminal, drop an approval the user has not answered or cut off a run. The work outlives the server, and the next server picks it up where the last one left it.

## What outlives a server on its own

- **Runs.** Each run's wrapper owns its payload and records the outcome in receipts; the engine reconnects and catches up ([experiments](experiments.md#the-engine)).
- **SSH logins.** A ControlMaster with `ControlPersist` on Unix, the detached `orx ssh-service` on Windows ([remote](remote.md#transport-and-the-auth-breaker)).
- **The store.** Turns, queued messages, notices and leases are rows ([storage](storage.md)).

Everything else a server starts is its child and would die with it. The children that hold work are kept instead.

## Keepers

A **keeper** (`crates/orx-keep`) holds one child for the server: it starts the child in a process group of its own, owns the child's stdio or terminal, retains its output, and serves one attachment at a time over a loopback socket with a token. It never parses what the child says. [FROZEN.md](../crates/orx-keep/FROZEN.md) is its contract, because every keeper running anywhere meets every later server.

```
orx up N   ─╮                         keeper ── child
orx up N+1 ─┴─ attachment (one) ──────  holds stdio / PTY / ConPTY
               input: whole frames      retains output; ends the child
               ack · resize · signal    when no server attached for a while
```

- **Kept:** resident agents and OpenCode servers, per-turn agents, the commands orx runs for ACP agents, and project terminals.
- **Not kept:** one-shots (titles, starter prompts, compaction summaries), probes, catalog reads, sign-in servers and the terminals of setup and sign-in flows. They are short and run as the server's own children. A restart waits up to five seconds for one-shots, then ends them, since they are cheap to ask again; the rest stay blocking work for a restart.
- **The process.** On Unix the keeper is the extracted static `orx-host` (`orx-host keep`), so neither an update nor an unmounted AppImage removes its code; without one, `orx keep`. On Windows it is `orx keep`, which an update renames aside rather than replaces, inside a Job Object that ends its child when the keeper ends.
- **A keeper never outlives its purpose.** Dropping a kept child without handing it off stops it, as `kill_on_drop` would (`src/keep.rs`). A keeper with no server attached for its orphan timeout ends its child, so a server that never comes back leaves nothing running for long. A dead keeper takes its child with it.
- **Each keeper has a directory** under the data directory's `keep/`, owner-only: its lock, endpoint, exit receipt, log, and the server's `owner.json`, which names whose child it is and what a successor needs to adopt it.

## Offsets and acknowledgement

**A server acknowledges output only once everything before it is durable.** Every byte a child writes has an offset. A keeper of agent output retains everything not yet acknowledged; a successor that attaches receives exactly what its predecessor had not made durable, and replays it as if it were new. A terminal's keeper retains only the newest output, as scrollback, and never makes the child wait.

Durable means reflected in the store or in `owner.json`: a transcript flush with its turn checkpoint during a turn, a recorded segment or event between turns. Output an owner holds only in memory, like a segment Claude Code starts on its own before a turn claims it, stays unacknowledged.

## Turns resume from checkpoints

**Each transcript flush commits the assistant message and the turn's checkpoint together:** the keeper, the offset of the last line applied, and the state the harness needs to go on from there. Committed apart, a successor would apply a line twice or skip one.

When a server starts, every turn still running resumes if its keeper is alive and its checkpoint readable: the harness attaches, skips to the checkpoint's offset, restores its state and continues the same turn, without sending the prompt again. Only a server runs turns and a starting one holds the engine lock, so any lease it finds is a dead server's, and a crashed server's turns resume at once too. Anything else, a dead keeper, a missing or unreadable checkpoint, a harness that cannot resume, falls back to [recovery](agents.md#turns-survive-restarts): a failed turn with Continue or Retry, except that a prompt the agent never received is sent again once on its own. Nothing the agent received is replayed.

- **A side effect is checkpointed after it happens.** An answer written to an agent (an approval, a file read, a terminal result) is followed by a checkpoint. A successor that resumes before it answers the request again, which an agent ignores; one that never answered would leave the agent waiting forever.
- **Requests in flight survive.** An agent's request that the checkpoint holds unanswered is answered by the successor; an approval reappears as the same card. orx's own requests still awaiting an answer are in the checkpoint too, so their answers are recognized, and a successor numbers new requests where its predecessor's cannot collide.
- **Unread steers stay with the turn.** A steer written to an agent is in the checkpoint until the agent reads it; a successor holds it again, so the agent reading it records it in place rather than the queue running it a second time.

## Idle agents are adopted

A starting server reads each live keeper's `owner.json` and gives the child back to its harness host, which treats it like one it spawned: the next turn reuses it, settings changes still restart it, and the idle reaper still releases it. A child whose chat is gone, or whose record no harness understands, is stopped; a keeper whose directory, record or lock cannot be read is left running and reported, and a directory is only removed once its lock is confirmed released.

**What an agent's environment names outlives a server.** The port, the server's bearer, the key `orx mcp` tokens derive from, and each child's approval token are kept per lineage of servers, not per process: a successor reads them back, and a fresh set is drawn only when no kept child survives to need the old one. A record that is there but cannot be read keeps the server from starting, since a fresh set would cut those children off.

## The gap between servers

**An agent's call to orx waits out a restart.** `orx mcp` and the `orx` CLI retry a refused or dropped connection for as long as a restart takes, then fail with the reason. A call orx may already have acted on is retried only when repeating it is safe; an approval is posted again by its tool call and finds its card.

A starting server accepts agents' calls only once it has adopted their children and turns, so a call never meets a server that has not yet heard of its chat.

## Restarting

**A restart hands its children off; a quit stops them.** Restarting into an update, from the update banner or on its own once one is due, stops admitting requests, waits for those in flight, then hands off: no kept child is stopped, leases are released, and the server exits or execs into the new binary ([dashboard](dashboard.md#restarting-into-an-update)). Running turns, queued messages, approvals, an agent's `wait` and terminals no longer hold a restart back. A turn that cannot resume stops as for a quit, its agent with it, so the successor adopts no agent still doing work its turn gave up. Quitting the app or interrupting `orx up` stops every kept child, as before. A crash hands off by default: its keepers wait out their orphan timeout for a successor. So does a desktop window that goes away without quitting: its server hands off and exits ([desktop](desktop.md#keeping-the-window-through-an-update)).

## Terminals

A project terminal has an id the page chose and keeps in its tab, and lives in a keeper. Opening the tab's socket attaches to that terminal when it exists and starts it when it does not, replaying its scrollback first, so a reload or a restart returns to the same shell. Closing the tab ends it. The server stays attached to every live terminal so its keeper never sees it orphaned while the page is closed.

## Remote machines

**A persistent host restarts the same way.** Updating a remote installs the laptop's build beside the running host and asks it to restart into it; its keepers carry its agents, turns and terminals across, and it keeps its instance name, so the laptop reattaches to the same host rather than offering a new one ([remote](remote.md#the-persistent-host)). A laptop's restart never touches its hosts: its gateways reattach to the sessions it had open. Versions still move together, since the dashboard protocol must match exactly: the laptop brings its hosts to its own build.
