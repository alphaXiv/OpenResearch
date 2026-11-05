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

An idle chat can change its agent from the model picker. The switch is a
settings change and a transcript row, with no model call. Messages record the
harness that owns their native session ids. Returning to a harness resumes
its last session on the active branch and supplies only the conversation
since its last reply, with that reply's ref and a brief headline as the resume point;
a new harness gets the active conversation. Queued
messages must be sent or removed before switching. A chat retains its saved
ACP launch when switching away and back; changing between two ACP launches
requires a new chat.
Recovery after a switch uses the selected agent's settings and starts a new
turn, preserving the original failure and the harness that owns it. A native
thread created before delivery does not count as having seen the request.
Saved context stays available until the native thread confirms receipt.
The replayable request excludes agent-specific context; each launch rebuilds
the handoff for the selected agent. An empty branch carries no discarded history.

The handoff is rendered when the next turn starts, so worktree changes made
while the chat was idle are visible. The byte budget prioritizes user requests
and their message refs, then the latest answer, and fills remaining space with
older answers when they fit, or turn headlines and tool counts. Selected
messages stay in conversation order, with the answering agent, switches and failed
turns identified. Long requests keep their beginning and end. Tool logs stay
behind refs: `read {ref: "msg_…"}` reads visible message parts, tool calls and
attachment paths, excluding reasoning even inside nested agent calls.
`read {before: "msg_…"}` pages backward on the active branch. The same builder
supplies native-session recovery, compaction's fallback input, ACP branch
reseeding and side chats.
Agents without an exposed MCP tool send the same `read` call through
`orx mcp` over stdio; the chat's environment supplies its identity.
The default ceiling is 24,000 UTF-8 bytes, a conservative upper bound on text
tokens; when the catalog reports the target context window, the ceiling also
stays within ten percent of it. If even the user-ref index cannot fit, the
handoff directs the agent to page through the conversation.

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
up, while its sessions and history stay out of the user's own. Codex shares
home entries by default so relative configuration paths keep working; known
session stores, databases, memories and runtime state are excluded. The links
are reconciled before each turn, including when reusing a running Codex
process. **orx never edits the user's agent configuration.** What it needs to
add, such as hooks, an MCP server or sandbox roots, goes in through the agent's
command line or a settings file of orx's own.

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
  is how `orx exp run` records which chat launched a run and how an agent asks
  to be woken when a run ends.
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
  makes the `approve` tool of its own MCP server, `orx mcp`, Claude's
  permission tool. It forwards each decision to the owning dashboard over
  loopback and blocks until the user answers. The dashboard accepts it only with the per-chat
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

Every agent whose harness takes an MCP server gets orx's own, `orx
mcp`. It speaks stdio, the one transport every harness accepts (ACP
requires it), and relays to the owning dashboard, which knows the calling
chat by a token derived from a key that lives only in that dashboard's
process. Through it an agent delegates. It names a versioned model, and orx
picks a route that can run it, unless the agent names one: the vendor's own
agent first, then flat-rate plans, direct APIs, and resellers last. The same
model is spelled differently by every agent and provider, so routes are joined
on a canonical id derived from each spelling.

**A delegate is a side chat of the chat that started it,** working in the
same worktree, unless it asks for a worktree of its own and stands as an
ordinary chat. It starts with only the task it was given, runs with a
permission mode no broader than its delegator's, and records who delegated to
it, which is how its depth is counted. Stopping a chat, by the user's Stop or
an agent's `cancel`, stops every delegate below it. An agent can also start a chat it does not hear from: that chat is
no delegate and stands as the user's, as a delegate the user kept does. A
retry of the same request finds the chat the first attempt started, by an id
derived from the request.

**Any chat can subscribe to any other.** A subscription tells its subscriber
each finished turn of its target, and each question the target asks, as the
first message of the subscriber's next turn. Pending chat news arrives in one
visible turn, each piece kept in the transcript as its own message. Run results
use the engine's durable outbox in separate hidden turns; chat-news batching
never reads run wakeups or results. Both wait for the chat to be idle and keep
delivery claims to avoid duplicates. A turn that ends while its own delegates
are still working is not an answer yet. An agent can collect the same news inside
its turn with `wait`, which marks it told. A delegator subscribes to its
delegates. Once the user writes in a delegate, its composer lets them keep it:
it stops reporting to the chat that started it and stands as that chat's side
chat, or on its own with its own worktree, and the delegator's later
`subscribe` or `wait` does not undo that. A side chat reports only when its
agent `send`s. Agents answer questions through orx; approvals stay with the user.

**Agents write to each other in the open.** A task, message or piece of chat
news that orx or another chat's agent writes into a chat reaches its agent
opening with `[orx]`, so the agent can tell it from the user. The transcript
records it with its kind and sender and keeps what the sender wrote, refs included;
only the agent reads the refs expanded and the instructions around them. The
dashboard shows it as the sender's message. Run results and monitoring notices
reach agents in hidden turns; the transcript records only the agent's reply.

**Messages travel by reference.** Each user message reaches its agent tagged
with a short ref from its id. A ref in a task or message, the user's included,
expands to the cited message, its chat and its attachments, so a delegate reads
the user's own words rather than a paraphrase. In the dashboard, a ref is a
link to that message wherever it is written, and a chat id an agent writes is a
link to that chat; Copy link on a message puts its ref on the clipboard, and the
composer shows a pasted ref as a chip. An agent can read any chat. It can message or answer
its own delegates, and any other chat whose permission mode orx can read and is
no broader than its own, and it can stop its own delegates.

**Nesting is deliberate.** By default a delegate cannot delegate in turn. A delegation can let its delegate delegate in turn, a setting
changes the default, and every delegate is told how many levels down it is.

Where orx's server is present, the harness's own sub-agent tools are off
(Claude's Agent and Task tools, Codex's multi-agent feature, OpenCode's task
tool), so sub-agent work goes through orx and can run any model. A delegate
on Claude Code also gets the prompt-cache lifetime Claude gives its own
sub-agents, not the main conversation's longer one. Harnesses without the
server keep their native sub-agents, which appear as parts nested under the
call that started them, read from the agent's native records.
