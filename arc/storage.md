# Where state lives

orx keeps a researcher's work on their own machine. Projects, experiments, runs, chats and transcripts are kept nowhere else ([what leaves the machine](#what-leaves-the-machine) lists what is sent, and when), so the copy on disk is the only one, and every orx process on the machine reads and writes it at once: the dashboard and its run engine, the desktop app, a terminal `orx`, an agent's `orx`, a supervisor an older orx left watching a job.

## Three directories

| Directory | Default location | Holds |
| --- | --- | --- |
| config | `~/.config/openresearch` | settings, credentials, the SSH hosts saved in orx, the user's OpenResearch instructions (`AGENTS.md`), the telemetry outbox, the locks that cover the data directory as a whole |
| data | `~/.local/share/openresearch` | the store, repositories, worktrees, agent homes, run logs, artifacts, attachments, per-run state, and the keepers of agents and terminals that outlive a server ([restarts](restarts.md)) |
| cache | `~/.cache/openresearch` | what orx can download again: managed agent copies, price lists, the model feed; also the repositories older versions kept here, which [migrations](#migrations-follow-paths) move out |

Config and data honour their XDG variable; data and cache take an environment override and a setting (`src/config.rs`, `store::data_dir`).

**The data directory is one relocatable unit.** Its location can come from the environment or from the settings file, and is resolved afresh each time, so a process opening the store after a move finds the new place. It is owner-only because the store holds scoped compute capabilities. The desktop app imports the overrides from the login shell, or it and a terminal `orx` would open different databases ([desktop](desktop.md)).

**The config directory never moves**, because the setting that points at the data directory lives there and a lock inside the data directory could not guard a move of it. `settings.json` has one writer (`src/telemetry.rs`), a locked read-modify-write that keeps the fields a newer orx wrote; a second writer would clobber them. The OpenResearch instructions in `AGENTS.md` are the user's own file: orx writes it only when they save it in Customize or approve an agent's proposal, and it leaves the machine only inside the playbook, to the chat's agent ([agents](agents.md#what-orx-gives-an-agent)).

Saved SSH passwords and key passphrases sit in the system's credential store, never in a file ([remote](remote.md#saved-hosts)). `~/.openresearch/env` sits outside all three: the variables and compute credentials saved in compute settings, fanned out to agents and launched runs, in the one format shells and older versions read. Because it reaches runs on remote backends, a secret that must stay on this machine, like Overleaf's or an OpenCode provider's API key, belongs in the config directory instead. A project's own variables live in the store and override it for that project.

**A project's settings override the machine's, one by one.** `settings.json` holds the machine's choices; a project's own sit in its store row, and a chat's literature sources in the chat's, and neither travels with a share. A setting left unset follows the machine's, so a reset deletes it rather than copying the machine's value. `orx` commands act on the project of the chat that runs them, else the one whose folder holds the working directory (`src/local/resolve.rs`).

## The store

A SQLite database in WAL mode (`src/store.rs`, `src/store/`) is orx's truth for everything that is not code or a file named below: projects, experiments, runs, chats and turns, notices, collection obligations, SSH host state, UI state. Many short-lived processes open it at once, so write transactions stay short and hold no network I/O.

**The schema only grows.** Every open that may write applies the whole schema, idempotently: missing tables are created, missing columns added, data fixes rerun. The rare backfill that cannot rerun is gated on `user_version` inside the transaction that does it, so a half-applied one never records itself as done. A process that cannot write, such as a sandboxed agent's `orx`, opens read-only and applies nothing. Users run old and new binaries against the same file, a dashboard left open for days beside a freshly updated CLI, so:

- a new column needs a default an old binary can live with;
- a column is never renamed, retyped or given a new meaning;
- an old binary must be able to keep writing rows a new binary reads.

A test holds the schema to the last release's columns (`tests/contracts/store-schema.json`). Retiring a table is the one removal: the open folds any rows still needed into the replacement, then drops it, on every open, since an old binary may recreate it.

**`min_reader_version` is the only fence.** Every open reads it before applying anything and refuses a floor it does not support. A breaking host-disk change raises it (`crates/orx-host-proto/FROZEN.md`). It cannot stop a running or shipped 0.2.x writer, so database changes stay additive while those may exist.

Older binaries write the same rows, which shapes what is derived from them. Chat search is a full-text index in the same database, kept by triggers so writes from older binaries keep it current, which means those triggers must never fail on stored data. A chat pins a concrete model, but a legacy row without one stays NULL on every read; only its next turn records a model ([agents](agents.md#switching-agents)).

**Spend is recorded where it happens,** with the cost its agent reported or a price from a list orx keeps; a sample nothing prices stays unpriced, never zero, and currencies are kept apart, never converted. A run on a backend that bills keeps the price it was quoted at launch ([experiments](experiments.md)).

## Code lives in Git

A project is a Git repository anywhere on disk, the user's own or one orx cloned; the store records only its path. orx shells out to `git` rather than embedding a Git library, so every repository stays readable with ordinary tools (`src/local/git.rs`). Each chat works in its own worktree in the data directory, so agents never edit the user's checkout. [Experiments](experiments.md) covers how branches relate.

**A chat without a project gets a scratch project orx keeps for it,** in the data directory's `scratch/`. That location is what marks it: the store has no flag, so an older orx shows it as an ordinary project named "No project", and that name marks it unnamed. Promoting it, or moving a chat into a new project, relocates the repository or worktree while every chat involved is held like a running turn, and keeps ids, chats and artifacts, so links in earlier messages keep working; orx commits nothing on the user's behalf. **A move survives a crash:** it is recorded before it touches a file, and the next start finishes one whose files all arrived and undoes any other. Every writer of a project row names only the columns it changes, and the store refuses a write that points a project back at its scratch folder, which an older orx holding a stale row would otherwise make. A request made during a turn is stored and carried out when it ends (`src/local/projects.rs`, `src/local/chat/project_move.rs`).

Per-project artifacts are a plain folder (`src/local/files.rs`). The filesystem is their truth: there is no registry and no upload step, and every path the API accepts must resolve inside the folder once symlinks are followed.

## Run logs are files

A run's output is an append-only file, never rows in the store, so reading it never contends for a database write lock and a chatty job cannot bloat the database or slow every other writer. It is written on the machine that ran the job and kept in the data directory once collected ([experiments](experiments.md)). The signals folded from it are a checkpoint file beside it, published whole and never replaced by one that knows less. Until a run's log and signals are collected, the store keeps that as a debt, and the host's copy is not cleaned up.

## Agent homes

Agents run in homes orx prepares under the data directory, with their own session stores and the user's sign-in ([agents](agents.md#isolation)); chats created before isolation keep resolving to the user's own stores. These homes hold live SQLite files, which is why a move stops idle agents first. Managed agent releases live in the cache; removing an agent deletes only what orx installed for it.

## Locks and leases

Locks are advisory file locks; a holder that dies releases its lock with its file handle.

| Lock | Held by | Guards |
| --- | --- | --- |
| lifecycle (config) | shared: most `orx` commands, `orx up`, the app, each 0.2.x supervisor; exclusive: `orx delete`, a storage migration | destroying or relocating shared state while anything runs |
| engine (`.orx-remote-<hash>.lock`, beside the data directory) | exclusive: each `orx up` for its whole life | one server owns a data directory's runs |
| run supervisor (`src/engine/locks.rs`) | a 0.2.x supervisor, or the engine once it adopts the run | one owner per run |
| data-directory move (config) | exclusive: a move; shared: file-deleting sweeps, the 0.2.x SSH service | a move starting under them |
| managed agents (config) | install, update and removal | shared runtimes and releases |
| run artifacts (data) | shared: submission; exclusive: deletion, orphan sweeps | a run's files while it is submitted |
| submission (data), one per experiment | submission | one live run per experiment unless forced |
| native deletion (data) | deleting chats with agent-native history | lookup, purge and row deletion across dashboards |

Narrower locks serialize settings writes, token refreshes and cancel requests beside the state they guard. Update locks belong to [desktop](desktop.md), host locks to [remote](remote.md).

**The lifecycle lock** is held for a command's whole life, and a headless `orx up` stays up while any run is unfinished, so it is held for as long as orx watches a job. It has a directory of its own so a sandboxed agent can be granted write access to it and nothing else in config, and its older path is the same file, so old and new binaries contend on one lock.

Leases are rows in the store, each with a way to lapse when its holder dies:

- a chat turn lease keeps one live turn per chat, renewed by heartbeat and expired by a TTL;
- a subscription claim keeps one sweep delivering an update, expired by a TTL;
- the move lease marks a data-directory move in progress for new turns, sweeps and deletions to check; the next process to find the move lock free clears one whose owner died.

## Moving the data directory

**A move never destroys the old tree before the new one is proven.** Only the dashboard moves it (`src/local/datadir.rs`), and only while nothing is active: it refuses under an environment override or while any turn, run, project operation or project terminal runs, then holds the move lock and lease so none starts.

The tree is renamed on the same filesystem, or copied and verified. Every absolute path recorded inside it is then rewritten (`src/local/storage.rs`), and the move fails if any reference to the old location survives. Only then does the persisted path swap. A copy keeps the old tree and reports it instead of deleting it, because a detached supervisor or a request that resolved the old path just before the swap may still be writing there.

## Migrations follow paths

Old layouts are recognised by where things are, never by a marker recording that a migration ran (`src/local/storage.rs`). A migration runs under the exclusive lifecycle lock; if other orx processes are running, it stops and asks the user to close them, having moved nothing. A renamed folder is renamed in place when only the old name exists, and left alone when both exist.

Detection repeats on every start, so an interrupted migration resumes on the next run. One that cannot tell which copy is right stops with every file preserved and asks the user to resolve it.

## What leaves the machine

**Code, experiments, runs, logs, chats, transcripts and artifacts go to openresearch.sh only when the user publishes a project.** Otherwise its API serves accounts, organizations, SSH keys, sandboxes and compute catalogs, plus the telemetry and feedback below. A test keeps research-state endpoints other than publishing out of `src/client.rs`; `src/jobs/rental.rs`, the box's `orx-host` and `src/telemetry.rs` call the API beyond that test's reach.

This list names each kind of traffic, who receives it and what it carries; it changes when a new kind leaves the machine. The exact hosts live beside the code that calls them, and the e2e suite refuses any request its `listedEgress` (`e2e/src/outside.ts`) does not name.

- **Compute.** Work the user launches goes to the backend they chose: a source snapshot and the run's environment, which on most backends includes `~/.openresearch/env`, the project's variables and any configured Hugging Face token. Logs and metrics come back. Compute settings check saved credentials against their providers.
- **The SSH public key.** Only when the user adds a key does orx send a public key to their account, creating the pair if it is missing. The private key never leaves the machine.
- **Remote network check.** Setting up Claude Code on a remote machine without an API key asks a public IP service, from that machine, for its address and hosting classification. Only the source IP reaches it, and a failed lookup never blocks setup.
- **Paper search.** Queries and paper identifiers go to public paper indexes (alphaXiv, OpenAlex, bioRxiv, PubMed) without credentials, except an API key the user sets for one. PDFs come from arXiv or alphaXiv.
- **alphaXiv.** Sign-in is a browser trip to alphaXiv whose token orx exchanges with openresearch.sh. alphaXiv's tokens stay owner-only in the config directory; the laptop lends its access token, never the refresh token, to SSH hosts attached to it ([remote](remote.md)). orx reads the user's public profile and library, and blank-project starter prompts are briefed from the profile, so it reaches the agent's model provider. `orx alphaxiv-mcp` forwards agents' research tool calls and their arguments to alphaXiv, and sends no transcript, project files, experiments or logs of its own; tool results, library notes included, reach the agent's model provider. The page loads avatars and thumbnails from alphaXiv.
- **Telemetry.** Official builds send opt-out usage events to the first-party API under a random install ID not tied to an account: commands, product events, agent setup states, model choices, token usage, and the research profile text an older onboarding submits. Events carry a hash that tells one chat's turns apart without naming the chat, and say whether the computer is signed in to alphaXiv or OpenResearch, never which account. On first launch a desktop app sends the download click ID its installer's page address carried, once, so an install can be matched to the download that led to it. Failures are reported as fixed categories, never raw output, local error text or account details. Source builds send nothing. Events queue in an outbox and send in the background, so telemetry never blocks or fails a command, and a change of the telemetry choice is itself recorded (`src/telemetry.rs`).
- **Feedback.** `orx feedback` sends an agent-written report with its context to the first-party API; the telemetry opt-out stops it, and signed in it carries the account token, which ties the install ID to the account. `agent-skills/orx-feedback/` tells agents what to leave out.
- **Sharing.** Nothing leaves on its own: sharing writes a `.orx` file the user sends themselves (`src/local/share.rs`), holding the project's branches, experiment tree, finished runs, logs, artifacts and the chats the user ticked. Anything that looks like a credential refuses the share; the code is not scanned, since it is what the user chose to send.
- **Published projects.** Publishing uploads exactly the file sharing would write, after the same scan, to openresearch.sh, which serves it to anyone with the link until the publisher deletes it; the upload names the local project ID, so publishing again replaces it.
- **Updates.** Each surface checks GitHub releases on its own schedule ([desktop](desktop.md)).
- **Prices.** `orx up` downloads a public model price list daily, and a run on a backend that bills reads its price, and later what it was billed, from that backend or openresearch.sh. The requests carry no research data, and costs never go to telemetry.
- **Run watching.** On by default where a cheap model follows from the chat's own: the agent's own, the chat's own free model, which already receives its turns, or Haiku or GPT Luna through the chat's provider or the user's own Claude Code or Codex. Otherwise it waits for the user to pick one. A bounded digest of a chat's runs (labels, state, progress, signals, selected log lines, the agent's brief and the watcher's own earlier notes) goes to that model's provider when something changes or a look is asked for. It never sends a full chat, repository or whole log, every field passes sharing's credential scan, and Settings turns it off ([experiments](experiments.md#the-watcher)).
- **Status report.** Under the same switch and model as run watching, each update sends the chat's previous report and a bounded digest of what happened since the last one: the chat's goal, the user's and agent's messages without tool output, delivered run notices, the runs' experiment and parent names, states, headline measures and results, the watcher's sentences and flags, the agent's milestone notes and the dashboard's locale. Every field passes sharing's credential scan ([experiments](experiments.md#the-watcher)).
- **Plan usage.** For each Claude Code and Codex sign-in that may be on a subscription, `orx up` asks the user's own agent how much of the plan is left, which reaches Anthropic or OpenAI with the user's sign-in ([agents](agents.md)). The answer stays in memory.
- **Model feed.** Every `orx up` asks openresearch.sh for the model feed at start and periodically. The request carries no credentials, account, install ID or research data.
- **Hosted free models.** The feed can name OpenAI-compatible endpoints whose models are free to everyone. A chat on one sends its turns there as it would to any provider; an endpoint at openresearch.sh or alphaxiv.org, or a subdomain of either, also receives the signed-in user's alphaXiv user ID, with no email or token, and no other host does.
- **Agent releases.** `orx up` checks daily for new releases of the agents it keeps current and downloads them, with their runtimes and packages, from each vendor's release servers and the package registries they publish to (`src/local/harness/managed/`), and installs OpenCode unasked while it is missing, for its free models. The requests carry no account or research data, and npm runs with an orx-owned prefix and configuration.
- **Installers the user starts.** Git for Windows comes from GitHub (`src/local/portable_git.rs`).
- **Integrations the user turns on.** Creating a project from a GitHub URL, publishing to GitHub, syncing a paper with Overleaf, with the user's browser session cookie, Trackio metrics, custom model endpoints, the agents' own model providers, which the agent talks to directly, and an Auto-mode reviewer model named in Settings, which receives the user's message and the action it reviews.
