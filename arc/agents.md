# Chats and agents

orx has no model of its own. A chat drives a coding agent the user already
has, signed in with their own account: Claude Code, Codex, OpenCode, Cursor,
Antigravity, or any agent that speaks the Agent Client Protocol (ACP). Each
agent has its own process model, event format, permission system and session
store, and each changes underneath orx on its own release schedule. orx has
to present one chat over all of them, keep the agent's hands off the user's
checkout and configuration, and never lose a turn because a process died.

## A chat

A chat belongs to one project and works in its own Git worktree of that
project ([storage](storage.md)). Its transcript is a tree of messages: a user
message and the assistant message that answers it form one turn, and
re-sampling a turn or editing an earlier message adds a sibling fork rather
than overwriting. Every message is a list of parts (text, reasoning, tool
calls with their output, prompts awaiting the user, status rows), and parts
nest, so a sub-agent's work sits inside the call that started it.

**orx's store is the system of record for transcripts; the agent's native
session is only context.** The agent keeps its own session so it can resume
with its memory intact, and orx records, on each message, which native session
it began from and ended at. Switching to another fork rewinds the agent to the
native session that fork ended at. Files do not rewind: forks share the chat's
worktree, so a fork starts from whatever the previous turn left on disk.

```text
message ─► persist the user message, reserve the turn
        ─► harness adapter: start or reuse the agent, send the prompt
        ─► native events ─► normalized parts ─► assistant message
                                              ├─ persisted on each flush
                                              └─ broadcast to the dashboard
```

A turn streams by rewriting its assistant message: each flush, a few times a
second at most, persists the whole message and broadcasts it as one event.
The dashboard replaces its copy and never has to merge deltas. Tool output
is capped head-and-tail so a chatty command cannot make every flush huge.

## Harnesses

A harness is one integration behind a common interface. A harness may offer
detection (is the CLI installed, is the user signed in, which models), chat
turns, and skill installation; none is required to offer all three. Adding
one is a new implementation and one registry entry, and the picker, the
detection sweep and the skill installer find it.

Detection runs in two passes because model catalogs are slow to probe. The
first answers installed and signed in from files and environment alone, so
the picker appears at once. The second fills in models and capabilities in
the background.

The providers differ in shape, and the interface absorbs three:

| Process model | Agents | How a turn runs |
| --- | --- | --- |
| resident child per chat | Claude Code, Codex, ACP agents | one long-lived process over a stdio protocol; a turn is one request, its events stream back |
| shared server | OpenCode | one `opencode serve` per dashboard; a turn posts to it and follows its event stream |
| child per turn | Cursor, Antigravity | a print-mode process per turn, resuming the native session by id |

A resident child is respawned with its native session resumed when a setting
it was started with changes, when it is interrupted, or when it crashes.
Prompts flow back in one of two ways. Some agents end their turn on a
question and take the answer as the next message. Others pause mid-turn and
take the answer over the live connection, so the same turn continues. Agents
that can accept input mid-turn take steering messages directly; for the rest,
a message sent during a turn waits in the chat's queue.

## Isolation

An agent runs in the chat's worktree with a configuration home orx prepares
inside its data directory. The user's sign-in, settings, instructions, plugins
and skills are linked into that home, so the agent behaves as the user set it
up, while its sessions and history stay out of the user's own. **orx never
edits the user's agent configuration.** What it needs to add, such as hooks,
an MCP server or sandbox roots, goes in through the agent's command line or a
settings file of orx's own.

Chats the user had in an agent's own CLI can be adopted. Those keep resolving
to the user's native store, and orx copies recent history only for display.

## What orx gives an agent

- **The playbook.** orx's system prompt, rendered with the project's current
  state, travels through each agent's instruction channel: a system-prompt
  file, developer instructions, or a file in the worktree that the first turn
  points to.
- **Skills.** orx's bundled skills and the skills the user uploaded are
  written into the worktree's native skills folder for that agent, fresh every
  turn, so they never drift from the binary. Skills the project or the user's
  agent already has are found by the agent itself. A shared Git exclude keeps
  orx's files out of the project's history.
- **The `orx` CLI.** The directory of the running `orx` is put first on the
  agent's `PATH`, and kept first even after the user's shell startup files
  run, so the agent runs the same version as the dashboard. The environment
  names the chat and the owning dashboard's port (plus a token, on a
  persistent remote host). That
  is how `orx exp run` records which chat launched a run, how an agent asks to
  be woken when a run ends, and how `orx agent spawn` opens a new chat in the
  project.
- **alphaXiv tools.** Claude Code, Codex, OpenCode and ACP agents always get
  a stdio server, `orx alphaxiv-mcp`, named `alphaxiv`. If Codex already
  defines that name, orx disables it for the thread and uses `orx_alphaxiv`;
  OpenCode always uses `orx_alphaxiv`, because it merges same-named servers
  across the user's config layers. The user's configuration stays untouched.
  Signed out, including on remote hosts, it forwards research calls without
  credentials to alphaXiv's anonymous MCP endpoint. Signed in, it refreshes
  this computer's alphaXiv token under the shared lock and exposes library
  reads plus requested folder creation and paper saves. Only the allowlisted
  tools reach the agent. The write tools carry an ask rule for Claude Code,
  Codex's app server and OpenCode, whose rule also covers code-mode calls. ACP
  agents get no per-tool rule and the older `codex exec` path cannot prompt,
  so there the agent's own policy decides.
  Cursor and Antigravity get no server and read the library with
  `orx alphaxiv library`.
  Sign-in changes respawn a resident child on its next turn, resuming its native
  session so the tool list changes without losing context. Tokens stay in orx.
- **Autonomy.** How much the agent may decide alone rides every turn as a
  block of context, independent of the harness, so changing it never restarts
  the agent.

## Permissions

Each agent keeps its own permission modes, and orx exposes the modes the
harness advertises. When an agent wants approval for a tool call, the request
becomes a prompt part in the transcript and waits for the user. Where it waits
depends on the agent:

- Agents that ask their client (Codex's app server, OpenCode's server, ACP)
  hold the request on the live connection until the user answers.
- Claude Code in headless mode cannot prompt. For approvals mid-turn, orx
  registers a small MCP server, `orx mcp-gate`, as Claude's permission tool.
  It forwards each decision to the owning dashboard over loopback and blocks
  until the user answers. The dashboard accepts it only with the per-chat
  token the child was spawned with, a new token for each respawn, because the
  endpoint grants permissions.
- Plan mode puts a small automatic policy in front of the user: read-only
  commands and web lookups pass, file edits are refused with a request to
  present the plan, and anything else asks.

**A held request never allows by accident.** A request outliving its turn is
denied, an interrupted turn denies everything it was waiting on, a transport
error denies, and an unanswered request is denied after a long timeout. The
turn's no-output watchdog ignores a turn blocked on the user, whose thinking
time is unbounded. Separately, a hook or plugin per agent stamps each shell
command with the model that issued it, so a run launched from that command
records which model asked for it.

## Turns survive restarts

A turn is a row with a state and a **delivery state**: whether its prompt was
never sent, rejected, accepted, or of unknown fate. While running, a turn
holds a per-chat lease in the store with a heartbeat, so two dashboards on one
database never run two turns in the same chat.

When a dashboard starts, or a lease expires, every turn still marked as
running has no process behind it. Each becomes a failed turn with an explicit
recovery action chosen by its delivery state, shown in the transcript:

```text
not sent, rejected   ─► retry     replay the same prompt; the agent never saw it
accepted, unknown    ─► continue  ask the agent to carry on; replaying could
                                  repeat work it already did
```

**Nothing is replayed without the user.** A continued turn resumes the native
session; if that session is gone, the agent gets a bounded snapshot of orx's
transcript instead, with instructions not to repeat completed tool actions.
Queued messages are persisted before they are acknowledged and run in order
as each turn ends, so a restart loses none of them; Stop clears the queue. A
turn silent for long enough is interrupted rather than left holding the chat
forever.

## Sub-agents and new chats

An agent's own sub-agents (Claude's tasks, OpenCode's child sessions,
Antigravity's spawned conversations) appear as parts nested under the call
that started them, read from the agent's native records. An agent can also
open an independent chat in the same project with `orx agent spawn`, which
goes through the dashboard's API like a user's request and becomes an
ordinary chat.
