# Chats and agents

orx has no model of its own. A chat drives a coding agent the user already has, signed in with their own account: Claude Code, Codex, OpenCode, Cursor, Antigravity, or any agent that speaks the Agent Client Protocol (ACP). Each has its own process model, event format, permission system and session store, and each changes on its own release schedule. orx presents one chat over all of them, keeps the agent's hands off the user's checkout and configuration, and never loses a turn because a process died.

`src/local/chat/` owns chats and turns, `src/local/harness/` the per-agent adapters, and `src/local/` the process hosts (`claude.rs`, `codex.rs`, `opencode.rs`, `acp/`).

## A chat

A chat belongs to one project and works in its own Git worktree ([storage](storage.md)). Its transcript is a tree of messages: a user message and its answer form a turn, and re-sampling a turn or editing an earlier message adds a sibling fork. A message is a list of parts (text, reasoning, tool calls, prompts, status rows), and parts nest, so a sub-agent's work sits inside the call that started it.

**orx's store is the system of record for transcripts; the agent's native session is only context.** Each message records the native session it began from, the one it ended at, and the harness that owns them, so selecting a fork rewinds the agent to that fork's session. Files do not rewind: forks share the worktree, so a fork starts from whatever the previous turn left on disk.

A turn: persist the user message and reserve the turn → the adapter starts or reuses the agent and sends the prompt → native events normalize into parts → each throttled flush persists the whole assistant message and broadcasts it as one event. The dashboard replaces its copy and never merges deltas, so tool output is capped head-and-tail to keep flushes small.

## Harnesses

A harness is one integration behind the `Harness` trait (`src/local/harness/mod.rs`), offering any of detection, chat turns and skill installation. Adding one is one implementation and one registry entry. Detection runs in two passes because model catalogs are slow: the first answers installed and signed in, so the picker appears at once; the second fills in models in the background.

| Process model | Agents | A turn |
| --- | --- | --- |
| resident child per chat | Claude Code, Codex, ACP agents | one request over a stdio protocol; events stream back |
| resident server per chat | OpenCode | posts to `opencode serve` in the chat's worktree; 1.x and 2.x have separate adapters |
| child per turn | Cursor, Antigravity, older Codex (`codex exec`) | a print-mode process resuming the native session by id |

A resident agent restarts with its native session resumed when a setting it started with changes or when it crashes. Resident children also restart after an interrupt, and idle ones are released (`src/local/agent_lifecycle.rs`); the next message resumes them.

An agent either ends its turn on a question and takes the answer as the next message, or pauses and takes the answer over the live connection. Agents that accept input mid-turn take steering directly; otherwise a message sent during a turn waits in the chat's queue.

Compaction is native where the harness supports it; otherwise a one-shot summary of the conversation reseeds a fresh native session.

## Switching agents

**A switch is a settings change and a transcript row, with no model call.** Only an idle chat with no queued messages can switch. A chat keeps its saved ACP launch across a switch away and back; moving between two ACP launches needs a new chat.

A returning harness resumes its last session on the active branch and gets only the conversation since its last reply, with that reply's ref and headline as the resume point. A new harness gets the active conversation.

One builder (`src/local/chat/handoff.rs`) renders this handoff and also feeds native-session recovery, compaction's fallback input, ACP branch reseeding and side chats:

- It renders when the next turn starts and is never stored with the request, so it shows the current worktree and a replay rebuilds it for the agent then selected.
- It is bounded, with user requests and their refs outranking answers.
- Tool logs stay behind refs, readable with the `read` tool of `orx mcp`, which excludes reasoning. An agent without that tool pipes the same call to `orx mcp`.
- A native session has seen a request only once delivery is confirmed.
- Saved compaction context applies only to its branch; rewinding before it rebuilds the selected conversation, and an empty branch carries no discarded history.

Recovery after a switch starts a new turn under the selected agent's settings, preserving the original failure and the harness that owns it.

## Isolation

An agent runs in the chat's worktree. **orx never edits the user's agent configuration.** What it adds (hooks, an MCP server, sandbox roots) goes through the agent's command line or a settings file of orx's own. One exception stands: starting OpenCode rewrites the `orx` skill shim in the user's OpenCode skills directory. orx's worktree files are Git-excluded, and a project's tracked agent config wins: OpenCode's config moves aside for a tracked `opencode.json`, and an Antigravity turn fails on a tracked `.agents/hooks.json`.

Homes (`src/local/native_store.rs`) let an agent behave as the user set it up while its sessions stay out of the user's own:

| Agent | orx's own | Reused from the user |
| --- | --- | --- |
| Claude Code, Cursor | config home | an allowlist of entries, linked in |
| Codex | config home | every entry, linked in, except known session stores, databases, memories and runtime state, so relative configuration paths keep working |
| OpenCode | session database | configuration, read in place |
| Antigravity, ACP agents | nothing | home and session store |

Links are reconciled when an agent starts, and before every Codex turn even on a running process. Claude Code and Codex chats from the user's own CLI can be adopted (`src/local/native_chats.rs`): they keep resolving to the user's native store, and orx's copy of recent history is for display only.

## Local models

OpenCode runs local inference over connections to OpenAI-compatible servers (`src/local/local_models.rs`). They live in `local-models.json` in the data directory, owner-only, and reach OpenCode as inline configuration on every spawn, so the user's OpenCode files stay untouched.

- **Loopback only,** relative to the machine running `orx up`; with a remote host, the model server runs on that host.
- **No silent fallback to a cloud model.** A chat pins OpenCode's title and compaction models to its selected model, and the start fails if the effective configuration disagrees.
- **Offered only when servable.** A loopback provider, orx's or the user's own, appears in the picker only while its server advertises the model, and OpenCode's provider allow and deny lists stay in force.

## What orx gives an agent

- **The playbook.** `SYSTEM_PROMPT.md`, rendered with the project's state in `src/local/opencode.rs` for every harness, travels through each agent's instruction channel: a system-prompt file, developer instructions, a config entry, or a worktree file the first turn points to.
- **Skills.** Bundled (`agent-skills/`) and user-uploaded skills are written into the worktree's native skills folder before each turn, so they never drift from the binary; OpenCode gets them when its server starts. Skills the project or the user's agent already has are found by the agent itself.
- **The `orx` CLI.** The running `orx`'s directory goes first on `PATH`, and on Unix shell hooks keep it first after the user's startup files, so the agent runs the dashboard's version. The environment names the chat, the dashboard's port and the server's bearer, which is how `orx exp run` records its chat.
- **alphaXiv tools.** `orx alphaxiv-mcp` (`src/commands/alphaxiv_mcp.rs`) passes an allowlist of tools to Claude Code, Codex, OpenCode and ACP agents; Cursor and Antigravity use `orx alphaxiv library`. Tokens stay in orx ([storage](storage.md)). A sign-in change restarts a resident agent on its next turn with its session resumed. Write tools carry an ask rule, but ACP agents get no per-tool rule and `codex exec` cannot prompt, so there the agent's own policy decides.
- **Turn context.** Autonomy and the goal ride the turn as context blocks, so changing them never restarts the agent.

## Permissions

Each agent keeps its own permission modes, and orx exposes those the harness advertises. A tool call needing approval becomes a prompt part:

| Agents | How a request waits |
| --- | --- |
| Codex app server, OpenCode, ACP | held on the live connection |
| Claude Code, Antigravity | headless, so a bridge relays to the owning dashboard and blocks: Claude's `approve` tool on `orx mcp`, Antigravity's `orx antigravity-gate` hook |
| Cursor, `codex exec` | nothing waits: print mode cannot prompt, so the mode maps to CLI flags |

The bridge endpoint grants permissions, so it takes a per-child token that each respawn replaces. Plan mode puts a small policy in front of the bridge: read-only commands and web lookups pass, file edits are refused, and anything else asks. The read-only check is an allowlist (`src/local/harness/plan_gate.rs`), so a new read-only `orx` verb stays gated until added there.

**A held request never allows by accident.** A request outliving its turn is denied, an interrupted turn denies everything it was waiting on, a transport error denies, and the bridge denies an unanswered request after a long timeout. The no-output watchdog ignores a turn blocked on the user, whose thinking time is unbounded.

Where an agent offers a hook or plugin, it stamps each shell command with the issuing model, so a run records which model launched it (`src/commands/invocation_gate.rs`).

## Turns survive restarts

A turn is a row with a state and a **delivery state**: its prompt was never sent, rejected, accepted, or of unknown fate. A running turn holds a per-chat lease with a heartbeat, which keeps one turn per chat and exposes a turn whose process died ([storage](storage.md)).

When a dashboard starts or a lease expires, every turn still marked running has no process behind it. Each becomes a failed turn with a recovery action chosen by its delivery state:

| Delivery state | Action | Effect |
| --- | --- | --- |
| not sent, rejected | retry | replays the prompt; the agent never saw it |
| accepted, unknown | continue | asks the agent to carry on; replaying could repeat work |

**Nothing is replayed without the user.** An adapter retries on its own only before the prompt is accepted, within a bounded budget. A continued turn resumes the native session; if that session is gone, the agent gets a bounded snapshot of orx's transcript with instructions not to repeat completed tool actions.

Queued messages are persisted before they are acknowledged and run in order as each turn ends, so a restart loses none; Stop clears the queue. A quitting dashboard starts no turn, queued ones included, and gives up its leases on the way out, so the next start recovers its turns at once. Adapters that read a child's output interrupt a turn silent past a generous watchdog; OpenCode's adapters have none.

## Sub-agents and new chats

Every agent whose harness takes an MCP server gets `orx mcp` (`src/commands/mcp.rs`, tools in `src/commands/up/agent_tools.rs`). It speaks stdio, the one transport every harness accepts, and relays to the owning dashboard, which knows the calling chat by a token derived from a key that lives only in that dashboard's process.

Through it an agent delegates (`src/local/chat/delegation.rs`). It names a versioned model, and orx picks a route unless the agent names one: the vendor's own agent first, then flat-rate plans, direct APIs, and resellers last. Every agent and provider spells a model differently, so routes join on a canonical id derived from each spelling (`src/local/model_routes.rs`).

**A delegate is a side chat of the chat that started it,** in the same worktree, unless it asks for its own and stands as an ordinary chat. It starts with only its task, runs with a permission mode no broader than its delegator's, and records who delegated to it, which counts its depth. A harness with no permission modes takes a delegate only from a chat that may do anything. Stopping a chat stops every delegate below it. A chat an agent starts without hearing from it is no delegate and stands as the user's. A retry finds the chat the first attempt started, by an id derived from the request.

**Any chat can subscribe to any other** (`src/local/chat/news.rs`), hearing each finished turn and each question of its target. Pending news arrives in one visible turn once the subscriber is idle, or inside a turn through `wait`, and delivery claims prevent duplicates. A turn that ends while its own delegates work is not an answer yet. A turn that only answered the subscriber's own news is no news to it, or two chats subscribed to each other would wake each other forever. Run results travel separately, in hidden turns ([experiments](experiments.md)).

A delegator subscribes to its delegates. A delegate the user keeps stops reporting and stands as a side chat, or alone with its own worktree, and the delegator's later `subscribe` or `wait` does not undo that. A side chat reports only when its agent `send`s.

**Agents write to each other in the open.** What orx or another chat's agent writes into a chat reaches its agent opening with `[orx]`, so the agent can tell it from the user. The transcript keeps its kind, sender and what the sender wrote; only the agent reads refs expanded.

**Messages travel by reference.** A user message reaches its agent tagged with a short ref, unless a slash command needs its leading `/`. A ref in a task or message expands to the cited message, its chat and its attachments, so a delegate reads the user's own words rather than a paraphrase.

**Reach follows permissions.** An agent can read any chat. It can message or answer its own delegates and any chat whose permission mode orx can read and is no broader than its own, because a narrower chat steering a broader one would widen it. It can stop only its own delegates. Agents answer questions; approvals stay with the user.

**Nesting is deliberate.** By default a delegate cannot delegate. A delegation can allow it, a setting changes the default, and every delegate is told how many levels down it is.

Where orx's server is present, the harness's own sub-agent tools are off, so sub-agent work can run any model. Harnesses without it keep native sub-agents, read from the agent's records into parts nested under the call that started them (`src/local/chat/subagent.rs`).
