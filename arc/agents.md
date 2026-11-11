# Chats and agents

orx has no model of its own. A chat drives a coding agent the user already has, signed in with their own account: Claude Code, Codex, OpenCode, Cursor, Antigravity, or any agent that speaks the Agent Client Protocol (ACP). Each has its own process model, event format, permission system and session store, and each changes on its own release schedule. orx presents one chat over all of them, keeps the agent's hands off the user's checkout and configuration, and never loses a turn because a process died.

`src/local/chat/` owns chats and turns, `src/local/harness/` the per-agent adapters, and `src/local/` the process hosts (`claude.rs`, `codex.rs`, `opencode.rs`, `acp/`).

## A chat

A chat belongs to one project and works in its own Git worktree ([storage](storage.md)). Its transcript is a tree of messages: a user message and its answer form a turn, and re-sampling a turn or editing an earlier message adds a sibling fork. A message is a list of parts (text, reasoning, tool calls, prompts, status rows), and parts nest, so a sub-agent's work sits inside the call that started it.

**orx's store is the system of record for transcripts; the agent's native session is only context.** Each message records the native session it began from, the one it ended at, and the harness that owns them, so selecting a fork rewinds the agent to that fork's session. Claude Code keeps one session id for a whole chat, so a Claude reply also records the message it ended at, and switching, retrying or editing a fork starts Claude on a new session branched at the end of the turn the fork continues (`--resume-session-at` with `--fork-session`); a fork of the first turn starts fresh. The branch point is the newest reply that recorded its message; the handoff carries any turn after it, such as one interrupted before Claude wrote a message, and a branch with no such reply, or a branch point the CLI rejects, starts a new session that the handoff fills with the branch. Files do not rewind: forks share the worktree, so a fork starts from whatever the previous turn left on disk.

A turn: persist the user message and reserve the turn → the adapter starts or reuses the agent and sends the prompt → native events normalize into parts → each throttled flush persists the whole assistant message and broadcasts it as one event. The dashboard replaces its copy and never merges deltas, so tool output is capped head-and-tail to keep flushes small.

## Harnesses

A harness is one integration behind a common interface. A harness may offer
detection (is the CLI installed, is the user signed in, which models), chat
turns, and skill installation; none is required to offer all three. Adding
one is a new implementation and one registry entry, and the picker, the
detection sweep and the skill installer find it.

Detection runs in two passes because model catalogs are slow to probe. The
first discovers installations from files and environment; the second fills
in models and capabilities in the background. Claude Code verifies sign-in
through its CLI; profiles check theirs in parallel in the second pass. Adding,
renaming or removing an account, provider or endpoint re-checks only the agent
it changes.

**orx keeps the built-in agents current.** Installing one from the
dashboard downloads the vendor's release into orx's cache, checked against the
checksum the vendor publishes, when available, and leaves the user's `PATH`
and shell alone. Once a day, Claude Code, Codex (0.126 and later; older ones
have no updater) and OpenCode try their own updaters; when those cannot reach
the newest release, or the user's install fails to run, orx's copy stands in.
The newer of the two runs, a tie going to the user's install, and a chat's
running agent moves over at its next turn. An agent signed out on every
account is skipped until a sign-in shows it in use; one whose sign-in could
not be checked is still kept current. New OpenCode installs
use major 2; an existing major 1 still runs chats and offers an explicit upgrade
or OpenResearch's own copy before provider changes.
Agents orx starts run with Claude Code's updater off, so only this check
changes an install. Cursor, Antigravity and ACP presets also use cached vendor
releases; their updates replace orx's copy without invoking vendor installers.
DeepSeek Harness, MiniMax Code, Pi and the ZCode adapter use isolated npm prefixes
and orx's own Node.js LTS runtime, with that runtime and each agent's bins first
on PATH. The ZCode CLI itself still requires a separate installation. Vendor
scripts are read as release metadata only; none is executed.

**Model providers come through OpenCode.** Keys and provider names and icons
stay owner-only in `opencode-providers.json` in the config directory
([storage](storage.md)). Several keys for one provider use OpenCode 2 aliases,
each with its own model list and label; a catalog re-check refreshes those lists.
Custom endpoints stay in `local-models.json` in the data directory. OpenCode 2
browser sign-ins use its integration API and orx's isolated database. Each
attempt holds a server child and database lease until completion, cancellation,
failure or expiry, capped at ten minutes; removing its provider signs it out.
OpenCode's free models remain available until switched off.

Found agents can be turned off in Settings. Their models leave the picker and
sub-agent routes; their OpenResearch profiles stay available. Turning off found
OpenCode sign-ins keeps OpenResearch's provider records, endpoints and free
models. Removable managed installs ignore hidden-agent settings; they offer
Remove instead of a switch. Naming a turned-off route explicitly refuses the
delegation. The server alone applies this rule: delegation filters by it, and
each `/api/harnesses` entry carries its answer for the picker.

Remove deletes all OpenResearch-managed releases of Cursor, Antigravity or an
ACP preset, including its npm prefix. Pi's adapter also removes its managed Pi
runtime. Node stays because it also runs user-installed agents. Removal requires
running chats to finish or be stopped and closes idle ACP children first. A
removal error is reported for retry. The daily updater only maintains these
agents while a managed copy remains. User installations,
sign-ins, configuration and chat history stay; the shared `DEEPSEEK_API_KEY`
entry stays because it has no agent ownership record. Claude Code, Codex and
OpenCode cannot be removed here because their copies also back accounts and
providers.

Adding an agent commits on its last step. Profiles, providers and endpoints
saved on the way can be removed again when the add dialog is discarded.

The providers differ in shape, and the interface absorbs three:

| Process model | Agents | How a turn runs |
| --- | --- | --- |
| resident child per chat | Claude Code, Codex, Antigravity, ACP agents | one request over a stdio protocol; events stream back |
| resident server per chat | OpenCode | posts to `opencode serve` in the chat's worktree; 1.x and 2.x have separate adapters |
| child per turn | Cursor, older Codex (`codex exec`) | a print-mode process resuming the native session by id |

A resident agent restarts with its native session resumed when a setting it started with changes or when it crashes. Resident children also restart after an interrupt or a switch to another fork, and idle ones are released (`src/local/agent_lifecycle.rs`); the next message resumes them. A child with background tasks still running is not idle: Claude Code resumes on its own when one finishes, and orx records what it writes then as a turn with no user message. Until then orx refuses to switch, retry or edit a fork, which would stop the child and land its reply on the wrong branch. A task that never ends, such as a dev server, keeps its child until the chat stops or restarts it, on low-memory machines too, since releasing the child would kill the task.

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

Detection publishes the agent's configured model as `defaultModel`, including
models absent from its catalog. Without a known setting, it recommends the newest
Opus, Sol, or listed model, preferring base variants on a version tie. Signed-out
agents and pending catalogs cannot be selected. Cursor Auto is a selectable route;
Claude's `opusplan` is excluded, and saved `opusplan` selections resolve and persist
the configured concrete model or newest Opus before their next turn.
New dashboard selections store native model IDs.
Legacy null selections can still inherit the agent's configuration. Before a legacy
turn, Codex and ACP session models are preserved when known; Claude uses its
configured selection because transcript API names lose context variants.
Chat lists project legacy null selections from the cached agent default without
reading native history. Native recovery runs before a turn and persists its result;
an unreadable rollout fails that attempt without pinning a fallback model.

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

Claude Code and Codex profiles keep separate sign-ins in private homes under
`agents/profiles`, recorded in `agent-profiles.json`. They share the user's
settings and orx's native transcripts; Claude's account settings and credentials,
and Codex's authentication file, stay private. Claude's MCP servers are refreshed
from the user's configuration at launch. A chat records its profile, including
for delegates using the same harness; removing that account requires choosing
another agent before the chat can continue.

## Local models

OpenCode runs local inference over connections to OpenAI-compatible servers (`src/local/local_models.rs`). They live in `local-models.json` in the data directory, owner-only, and reach OpenCode as inline configuration on every spawn, so the user's OpenCode files stay untouched.

- **Loopback or HTTPS.** Loopback is relative to the machine running `orx up`; a remote endpoint requires HTTPS. Settings → Agents → Add agent → Custom endpoint configures the connection, its name and its icon.
- **Found by asking.** Add agent asks each known local server's usual loopback address for its models, offering the ones that answer, and asks for a key only once a server says it wants one.
- **No silent fallback to a cloud model.** A chat pins OpenCode's title and compaction models to its selected model, and the start fails if the effective configuration disagrees.
- **Offered only when servable.** A loopback provider, orx's or the user's own, appears in the picker only while its server advertises the model, and OpenCode's provider allow and deny lists stay in force.

## What orx gives an agent

- **The playbook.** `SYSTEM_PROMPT.md`, rendered with the project's state in `src/local/opencode.rs` for every harness, travels through each agent's instruction channel: a system-prompt file, developer instructions, a config entry, or a worktree file the first turn points to.
- **Skills.** Bundled (`agent-skills/`) and user-uploaded skills are written into the worktree's native skills folder before each turn, so they never drift from the binary; OpenCode gets them when its server starts. Skills the project or the user's agent already has are found by the agent itself.
- **The `orx` CLI.** The running `orx`'s directory goes first on `PATH`, and on Unix shell hooks keep it first after the user's startup files, so the agent runs the dashboard's version. The environment names the chat, the dashboard's port, the server's bearer and its data directory, which is how `orx exp run` records its chat and every `orx` the agent runs opens the store the dashboard serves.
- **alphaXiv tools.** `orx alphaxiv-mcp` (`src/commands/alphaxiv_mcp.rs`) passes an allowlist of tools to Claude Code, Codex, OpenCode, Antigravity and ACP agents; Cursor uses `orx alphaxiv library`. Tokens stay in orx ([storage](storage.md)); on an SSH host the attached laptop's sign-in counts ([remote](remote.md)). A sign-in change restarts a resident agent on its next turn with its session resumed. Write tools carry an ask rule, but ACP agents get no per-tool rule and `codex exec` cannot prompt, so there the agent's own policy decides.
- **Turn context.** Autonomy and the goal ride the turn as context blocks, so changing them never restarts the agent.

## Permissions

Each agent keeps its own permission modes, and orx exposes those the harness advertises. A tool call needing approval becomes a prompt part:

| Agents | How a request waits |
| --- | --- |
| Codex app server, OpenCode, Antigravity, ACP | held on the live connection |
| Claude Code | headless, so its `approve` tool on `orx mcp` relays to the owning dashboard and blocks |
| Cursor, `codex exec` | nothing waits: print mode cannot prompt, so the mode maps to CLI flags |

The bridge endpoint grants permissions, so it takes a per-child token that each respawn replaces. Plan mode puts a small policy in front of the bridge: read-only commands and web lookups pass, file edits are refused, and anything else asks. The read-only check is an allowlist (`src/local/harness/plan_gate.rs`), so a new read-only `orx` verb stays gated until added there.

**A held request never allows by accident.** A request outliving its turn is denied, an interrupted turn denies everything it was waiting on, a transport error denies, and the bridge denies an unanswered request after a long timeout. The no-output watchdog ignores a turn blocked on the user, whose thinking time is unbounded.

Where an agent offers a hook or plugin, it stamps each shell command with the issuing model, so a run records which model launched it (`src/commands/invocation_gate.rs`).

## Turns survive restarts

A turn is a row with a state and a **delivery state**: its prompt was never sent, rejected, accepted, or of unknown fate. A running turn holds a per-chat lease with a heartbeat, which keeps one turn per chat and exposes a turn whose process died ([storage](storage.md)).

When a dashboard starts or a lease expires, every turn still marked running has no process behind it, and a turn that finds its own lease gone stops. Each becomes a failed turn with a recovery action chosen by its delivery state:

| Delivery state | Action | Effect |
| --- | --- | --- |
| not sent, rejected | retry | replays the prompt; the agent never saw it |
| accepted, unknown | continue | asks the agent to carry on; replaying could repeat work |

**Nothing is replayed without the user.** An adapter retries on its own only before the prompt is accepted, within a bounded budget. A continued turn resumes the native session; if that session is gone, the agent gets a bounded snapshot of orx's transcript with instructions not to repeat completed tool actions.

Queued messages are persisted before they are acknowledged and run in order as each turn ends, so a restart loses none; Stop clears the queue. A quitting dashboard starts no turn, queued ones included, and gives up its leases on the way out, so the next start recovers its turns at once. Adapters that read a child's output interrupt a turn silent past a generous watchdog; OpenCode's adapters have none.

## Sub-agents and new chats

Every agent whose harness takes an MCP server gets `orx mcp` (`src/commands/mcp.rs`, tools in `src/commands/up/agent_tools.rs`). It speaks stdio, the one transport every harness accepts, and relays to the owning dashboard, which knows the calling chat by a token derived from a key that lives only in that dashboard's process.

Through it an agent delegates (`src/local/chat/delegation.rs`). It names a versioned model, and orx picks a route unless the agent names one: the vendor's own agent first, then flat-rate plans, direct APIs, and resellers last. Every agent and provider spells a model differently, so routes join on a canonical id derived from each spelling (`src/local/model_routes.rs`). A model the user turns off in Settings, or just its free tier, offers delegates no route.

**A delegate is a side chat of the chat that started it,** in the same worktree, unless it asks for its own and stands as an ordinary chat. It starts with only its task, runs with a permission mode no broader than its delegator's, and records who delegated to it, which counts its depth. A harness with no permission modes takes a delegate only from a chat that may do anything. Stopping a chat stops every delegate below it. A chat an agent starts without hearing from it is no delegate and stands as the user's. A retry finds the chat the first attempt started, by an id derived from the request.

**Any chat can subscribe to any other** (`src/local/chat/news.rs`), hearing each finished turn and each question of its target. Pending news arrives in one visible turn once the subscriber is idle, or inside a turn through `wait`, and delivery claims prevent duplicates. A turn that ends while its own delegates work is not an answer yet. A turn that only answered the subscriber's own news is no news to it, or two chats subscribed to each other would wake each other forever. Run results travel separately, in hidden turns ([experiments](experiments.md)).

A delegator subscribes to its delegates. A delegate the user keeps stops reporting and stands as a side chat, or alone with its own worktree, and the delegator's later `subscribe` or `wait` does not undo that. A side chat reports only when its agent `send`s.

**Agents write to each other in the open.** What orx or another chat's agent writes into a chat reaches its agent opening with `[orx]`, so the agent can tell it from the user. The transcript keeps its kind, sender and what the sender wrote; only the agent reads refs expanded.

**Messages travel by reference.** A user message reaches its agent tagged with a short ref, unless a slash command needs its leading `/`. A ref in a task or message expands to the cited message, its chat and its attachments, so a delegate reads the user's own words rather than a paraphrase.

**Reach follows permissions.** An agent can read any chat. It can message or answer its own delegates and any chat whose permission mode orx can read and is no broader than its own, because a narrower chat steering a broader one would widen it. It can stop only its own delegates. Agents answer questions; approvals stay with the user.

**Nesting is deliberate.** By default a delegate cannot delegate. A delegation can allow it, a setting changes the default, and every delegate is told how many levels down it is.

Where orx's server is present, the harness's own sub-agent tools are off, so sub-agent work can run any model. Harnesses without it keep native sub-agents, read from the agent's records into parts nested under the call that started them (`src/local/chat/subagent.rs`).
