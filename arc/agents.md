# Chats and agents

orx has no model of its own. A chat drives a coding agent the user already has, signed in with their own account: Claude Code, Codex, OpenCode, Cursor, Antigravity, or any agent that speaks the Agent Client Protocol (ACP). Each has its own process model, event format, permission system and session store, and each changes on its own release schedule. orx presents one chat over all of them, keeps the agent's hands off the user's checkout and configuration, and never loses a turn because a process died.

`src/local/chat/` owns chats and turns, `src/local/harness/` the per-agent adapters, and `src/local/` the process hosts (`claude.rs`, `codex.rs`, `opencode.rs`, `acp/`).

## A chat

A chat belongs to one project and works in its own Git worktree ([storage](storage.md)); a chat started without one gets a scratch project it can later promote. Its transcript is a tree of messages: a user message and its answer form a turn, and re-sampling a turn or editing an earlier message adds a sibling fork. A message is a list of parts (text, reasoning, tool calls, prompts, status rows), and parts nest, so a sub-agent's work sits inside the call that started it.

**orx's store is the system of record for transcripts; the agent's native session is only context.** Each message records the native session it began from and ended at, and the harness that owns them, so selecting a fork rewinds the agent to that fork's session, branching the native session where the agent allows it and replaying orx's transcript where it does not. Files do not rewind: forks share the worktree.

A turn: persist the user message and reserve the turn → the adapter starts or reuses the agent and sends the prompt → native events normalize into parts → each throttled flush persists the whole assistant message and broadcasts it as one event. The dashboard replaces its copy and never merges deltas, so tool output is capped to keep flushes small.

## Harnesses

A harness is one integration behind a common interface: detection (is the CLI installed, is the user signed in, which models), chat turns, and skill installation, none of them required. Adding one is a new implementation and one registry entry, and the picker, the detection sweep and the skill installer find it. Detection runs in two passes, because model catalogs are slow to probe: installations from files and environment first, then models and sign-in in the background.

The providers differ in shape, and the interface absorbs three:

| Process model | Agents | How a turn runs |
| --- | --- | --- |
| resident child per chat | Claude Code, Codex, Antigravity, ACP agents | one request over a stdio protocol; events stream back |
| resident server per chat | OpenCode | posts to `opencode serve` in the chat's worktree |
| child per turn | Cursor, older Codex | a print-mode process resuming the native session by id |

A resident agent restarts with its native session resumed when a setting it started with changes or when it crashes, and an idle one is released until the next message (`src/local/agent_lifecycle.rs`). **An agent with work still running is not idle,** whether a background task the agent will resume on its own or a command it left running, so orx does not release it, and refuses to switch its fork while a background task the agent will resume is still running.

**Retiring an idle agent gives it time to save a refreshed sign-in.** A token refresh can retire the token on disk before its replacement is saved, so retiring an idle process closes its input and waits before forcing it (`src/process.rs`).

An agent either ends its turn on a question and takes the answer as the next message, or pauses and takes the answer over the live connection. A message sent during a turn steers the agent where it accepts input mid-turn and waits in the chat's queue otherwise. Compaction is native where the harness supports it; otherwise a one-shot summary reseeds a fresh native session.

**Plan limits come from the agents themselves.** For each sign-in that may be on a subscription, orx asks the user's own agent through its own usage request and keeps the answer in `orx up`'s memory (`src/local/usage.rs`). Codex reads through the account's running app-server where one exists, since two processes refreshing one ChatGPT token can sign the user out. A failed read keeps the last answer beside the reason, never reporting no plan.

**orx keeps the built-in agents current.** Once a day it runs each agent's own updater, and when that cannot reach the newest release it downloads the vendor's release into its own cache, checked against the vendor's checksum where one is published. The newer copy runs. The user's `PATH`, shell and package manager stay alone, and no vendor install script is executed. `orx up` also installs OpenCode unasked, for its free models. A running chat moves to a new copy at its next turn (`src/local/harness/managed/`).

**Model providers come through OpenCode.** API keys, OpenAI-compatible endpoints and openresearch.sh's free models all reach the user as OpenCode providers, configured inline at each spawn so the user's OpenCode files stay untouched ([local models](#local-models)). A provider the user set up in their own OpenCode appears beside them as a row of its own.

**The server decides which routes exist.** Settings can turn off an agent, an account or a provider; detection and delegation both apply that from one answer, so the picker and a delegate never disagree. Removing an agent deletes only what orx installed for it; the user's installs, sign-ins, configuration and chat history stay.

## Switching agents

**A switch is a settings change and a transcript row, with no model call.** Only an idle chat with no queued messages can switch. A returning harness resumes its last session on the active branch and gets only the conversation since its last reply; a new harness gets the active conversation.

One builder (`src/local/chat/handoff.rs`) renders this handoff and also feeds native-session recovery, compaction's fallback, branch reseeding and side chats:

- It renders when the next turn starts and is never stored with the request, so it shows the current worktree and a replay rebuilds it for the agent then selected.
- It is bounded, with user requests and their refs outranking answers.
- Tool logs stay behind refs, readable through `orx mcp`.
- A native session has seen a request only once delivery is confirmed.

**A chat pins a concrete model.** A new selection stores the agent's native model id, so a chat keeps running what the user picked when the agent's default moves; older chats with no stored model resolve one before their next turn ([storage](storage.md#the-store)).

## Isolation

An agent runs in the chat's worktree. **orx never edits the user's agent configuration.** What it adds (hooks, an MCP server, sandbox roots) goes through the agent's command line or a settings file of orx's own. Two exceptions stand: starting OpenCode rewrites the `orx` skill shim in the user's OpenCode skills directory, and removing a provider found in OpenCode deletes it from the user's OpenCode, as the user asked. orx's worktree files are Git-excluded, and a project's tracked agent config wins over orx's.

Homes (`src/local/native_store.rs`) let an agent behave as the user set it up while its sessions stay out of the user's own:

| Agent | orx's own | Reused from the user |
| --- | --- | --- |
| Claude Code, Cursor | config home | an allowlist of entries, linked in |
| Codex | config home | every entry, linked in, except session stores, databases, memories and runtime state |
| OpenCode | session database | configuration, read in place |
| Antigravity, ACP agents | nothing | home and session store |

Profiles add more Claude Code and Codex accounts: each keeps its sign-in in a private home under `agents/profiles` and shares the rest. One-shots (a title, a watcher's look) run in a bare home of their own with the account's sign-in and nothing the user's setup would add to the prompt. Chats started in the user's own CLI can be adopted (`src/local/native_chats.rs`) and keep resolving to the user's native store.

## Local models

OpenCode runs local inference over connections to OpenAI-compatible servers (`src/local/local_models.rs`), kept owner-only in the data directory.

- **Loopback or HTTPS.** Loopback is relative to the machine running `orx up`; a remote endpoint requires HTTPS.
- **No silent fallback to a cloud model.** A chat pins OpenCode's helper models to its selected model, and the start fails if the effective configuration disagrees.
- **Offered only when servable.** A loopback model appears in the picker only while its server advertises it.

## What orx gives an agent

- **The playbook.** `SYSTEM_PROMPT.md`, rendered with the project's state for every harness, travels through each agent's instruction channel: a system-prompt file, developer instructions, a config entry, or a worktree file the first turn points to. Every turn reads all of it, so it carries only what every turn needs and routes the rest to skills and to files written beside it (`agent-playbook/`).
- **Skills.** Bundled (`agent-skills/`) and user-uploaded skills are written into the worktree's native skills folder before each turn, so they never drift from the binary. An agent sees each skill's description until it loads the skill, and a skill's `references/` only when its body sends it there, so what most turns read stays short.
- **The `orx` CLI.** The running `orx`'s directory goes first on `PATH` and stays there after the user's startup files, so the agent runs the dashboard's version. The environment names the chat, the dashboard and its data directory, which is how `orx exp run` records its chat and every `orx` the agent runs opens the store the dashboard serves.
- **alphaXiv tools.** `orx alphaxiv-mcp` (`src/commands/alphaxiv_mcp.rs`) gives agents an allowlist of research tools; tokens stay in orx ([storage](storage.md)), and on an SSH host the attached laptop's sign-in counts ([remote](remote.md)).
- **Saved variables.** The agent starts with `~/.openresearch/env` and its project's variables over it. Through `orx mcp` it can ask the user for more on a card in its chat (`src/local/chat/env_request.rs`); a secret's value never enters the transcript.
- **Turn context.** Autonomy and the goal ride the turn as context blocks, so changing them never restarts the agent.
- **Run attribution.** Each agent's shell commands carry the model that issued them, so a run records which model launched it.

## Permissions

Each agent keeps its own permission modes, and orx exposes those the harness advertises. A tool call needing approval becomes a prompt part:

| Agents | How a request waits |
| --- | --- |
| Codex app server, OpenCode, Antigravity, ACP | held on the live connection |
| Claude Code | headless, so a bridge on `orx mcp` relays to the owning dashboard and blocks |
| Cursor, `codex exec` | nothing waits: print mode cannot prompt, so the mode maps to CLI flags |

The bridge grants permissions, so it takes a per-child token that each respawn replaces. Plan mode puts a policy in front of it whose read-only check is an allowlist (`src/local/harness/plan_gate.rs`), so a new read-only `orx` verb stays gated until added there. Where orx reviews a request on the user's behalf, as OpenCode's Auto mode does, anything but a clear pass becomes the ordinary approval card.

**Bypassing permissions as root waits for the user.** Claude Code refuses to skip permissions as root without a sandbox flag, so orx offers that mode only after the user confirms the machine is disposable, and remembers the answer per machine.

**A held request never allows by accident.** A request outliving its turn is denied, an interrupted turn denies everything it was waiting on, a transport error denies, and Claude Code's bridge denies an unanswered request after a long timeout. The no-output watchdog ignores a turn blocked on the user.

## Turns survive restarts

A turn is a row with a state and a **delivery state**: its prompt was never sent, rejected, accepted, or of unknown fate. A running turn holds a per-chat lease with a heartbeat, which keeps one turn per chat and exposes a turn whose process died ([storage](storage.md)).

When a dashboard quits, starts, or finds a lease expired, every turn still marked running has no process behind it. Each becomes a failed turn with a recovery action chosen by its delivery state:

| Delivery state | Action | Effect |
| --- | --- | --- |
| not sent, rejected | retry | replays the prompt; the agent never saw it |
| accepted, unknown | continue | asks the agent to carry on; replaying could repeat work |

**Only a quit resumes on its own.** A dashboard that quits, for the user or an update, marks the turns it stops, and the next start recovers each before its chat's queue runs. A crash or a lapsed lease may come from the turn itself, so those wait for the user. An adapter retries on its own only before the prompt is accepted. A continued turn resumes the native session, or, if that is gone, gets a bounded snapshot of orx's transcript with instructions not to repeat completed actions.

**A failed turn says why in the user's terms.** The server classifies the agent's error; the chat names the cause, keeps the raw error in the details, and offers the action that fixes it, such as Sign in, which then recovers the turn by its delivery state.

Queued messages are persisted before they are acknowledged and run in order as each turn ends, so a restart loses none; a queued message leaves only when the user removes it. **Stop means stop:** a stopped chat and its delegates start no turn for run results or chat news until the chat's next turn starts.

## Sub-agents and new chats

Every agent whose harness takes an MCP server gets `orx mcp` (`src/commands/mcp.rs`, tools in `src/commands/up/agent_tools.rs`). It speaks stdio, the one transport every harness accepts, and relays to the owning dashboard, which knows the calling chat by a token derived from a key that lives only in that dashboard's process.

Through it an agent delegates (`src/local/chat/delegation.rs`). It names a versioned model, and orx picks a route unless the agent names one: the vendor's own agent first, then flat-rate plans, direct APIs, and resellers last. Every agent and provider spells a model differently, so routes join on a canonical id derived from each spelling (`src/local/model_routes.rs`).

**A delegate is a side chat of the chat that started it,** in the same worktree unless it asks for its own, and out of the sidebar. It starts with only its task, runs with a permission mode no broader than its delegator's, and records who delegated to it. Stopping a chat stops every delegate below it. A retry finds the chat the first attempt started, by an id derived from the request.

**Any chat can subscribe to any other** (`src/local/chat/news.rs`), hearing each finished turn of its target and each question a delegate asks, once the subscriber is idle or inside a turn through `wait`; delivery claims prevent duplicates. A turn that only answered the subscriber's own news is no news to it, or two chats subscribed to each other would wake each other forever. A delegator subscribes to its delegates. Run results travel separately ([experiments](experiments.md#notices)).

**Agents write to each other in the open.** What orx or another chat's agent writes into a chat reaches its agent opening with `[orx]`, so the agent can tell it from the user, and the transcript keeps who sent it.

**Messages travel by reference.** A user message reaches its agent tagged with a short ref, and a ref in a task or message expands to the cited message, its chat and its attachments, so a delegate reads the user's own words rather than a paraphrase.

**Any agent can reach any chat.** It can list chats: the top of the user's inbox or Working, a search that also reaches Done, or the chats linked to one by who started whom (parent, children, siblings, the whole tree). It can read, message or answer any chat, and stop only its own delegates; what the receiving agent does with a message is left to that chat's permission mode. Agents answer a delegate's questions; approvals, and questions from a chat that is no delegate, stay with the user.

**Nesting is deliberate.** By default a delegate cannot delegate; a delegation or a setting can allow it, and every delegate is told how deep it is.

Where `orx mcp` is present, the harness's own sub-agent tools are off, so sub-agent work can run any model, except where the agent can run them without asking. Harnesses without it keep native sub-agents, read from the agent's records into parts nested under the call that started them (`src/local/chat/subagent.rs`).
