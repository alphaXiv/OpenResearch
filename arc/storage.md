# Where state lives

orx keeps a researcher's work on their own machine. Projects, experiments, runs, chats and transcripts pass through no service, so the copy on disk is the only one, and every orx process on the machine reads and writes it at once: the dashboard and its run engine, the desktop app, a terminal `orx`, an agent's `orx`, a supervisor an older orx left watching a job.

## Three directories

| Directory | Location | Holds |
| --- | --- | --- |
| config | `$XDG_CONFIG_HOME/openresearch`, else `~/.config/openresearch` | `settings.json`, credentials for openresearch.sh, alphaXiv and Overleaf, the telemetry outbox, the locks that cover the data directory as a whole |
| data | resolved below | `orx.db`, `repos/`, `worktrees/`, `agents/`, `run-logs/`, `files/`, `chat-attachments/`, per-run state |
| cache | `ORX_CACHE_DIR`, the `cacheDir` setting, else `~/.cache/openresearch` | the legacy repository root, which [migrations](#migrations-follow-paths) empty |

**The data directory is one relocatable unit.** Its location resolves, most to least authoritative, from `ORX_DATA_DIR`, the `dataDir` in `settings.json`, `$XDG_DATA_HOME/openresearch`, and `~/.local/share/openresearch`. The setting is read fresh on every resolution, so a process opening the store after a move sees the new place. The directory is owner-only because the store holds scoped compute capabilities. The desktop app imports these overrides from the login shell, or it and a terminal `orx` would open different databases ([desktop](desktop.md)).

**The config directory never moves**, because the setting that points at the data directory lives there and a lock inside the data directory could not guard a move of it. `settings.json` has one writer, the locked read-modify-write in `src/telemetry.rs`, and every save keeps the fields a newer orx wrote; a second writer would clobber them.

`~/.openresearch/env` sits outside all three. It holds the variables and compute credentials saved in compute settings and fans out to agents and launched runs (`src/config.rs`), so a secret that must stay on this machine, like Overleaf's, belongs in the config directory.

## The store

A SQLite database in WAL mode (`src/store.rs`, `src/store/`) is orx's truth for everything that is not code or a file named below: projects, experiments, runs, chats and turns, notices, log-collection obligations, SSH host state, UI state. Many short-lived processes open it at once, each with a busy timeout, so write transactions stay short and hold no network I/O.

**The schema only grows.** Every open applies the whole schema: tables are created if missing, and each added column is an `ALTER TABLE ADD COLUMN` whose failure on an existing column is ignored. Data fixes are idempotent and run every time. The rare backfill that is not idempotent runs once in a single transaction, gated on a fresh `user_version` value, so a half-applied one never records itself as done. Users run old and new binaries against the same file, a dashboard left open for days beside a freshly updated CLI, so:

- a new column needs a default an old binary can live with;
- a column is never renamed, retyped or given a new meaning;
- an old binary must be able to keep writing rows a new binary reads.

A test holds the schema to the last release's columns in `tests/contracts/store-schema.json`.

Retiring a table is the one removal: the open folds any rows still needed into the replacement, then drops it. An old binary may recreate the table, so the fold runs on every open.

**`min_reader_version` is the only fence.** Every open reads it from `store_metadata` before applying anything and refuses a floor it does not support. A breaking host-disk change raises it (`crates/orx-host-proto/FROZEN.md`). It cannot stop a running or shipped 0.2.x writer, so database changes stay additive while those may exist.

Chat search is a contentless full-text index in this same database, kept by triggers on the message table so writes from older binaries keep it current; every writer runs those triggers, so they must never fail on stored JSON. Text, tool inputs, tool errors, shell output the user ran and subagent reports are indexed. Other tool output and reasoning deliberately are not. Membership in a session, project or branch comes from source rows at query time. History is indexed in bounded background batches behind a committed cursor, and completion is recorded once (`src/store/search.sql`, `src/store/search.rs`).

## Code lives in Git

A project is a Git repository anywhere on disk, the user's own or one orx cloned into `repos/`; the store records only its path. orx shells out to `git` rather than embedding a Git library, so every repository stays readable with ordinary tools (`src/local/git.rs`). Each chat works in its own worktree under `worktrees/<project>/<session>`, so agents never edit the user's checkout. [Experiments](experiments.md) covers how branches relate.

Per-project artifacts are a plain folder, `files/<project slug>/` (`src/local/files.rs`). The filesystem is their truth: there is no registry and no upload step, and every path the API accepts must resolve inside the folder once symlinks are followed.

## Run logs are files

A run's output is an append-only file, never rows in the store, so reading it never contends for a database write lock and a chatty job cannot bloat the database or slow every other writer. It is written on the machine that ran the job and kept under `run-logs/` once collected ([experiments](experiments.md)).

## Locks and leases

| Lock file | Held by | Guards |
| --- | --- | --- |
| config `locks/orx.lifecycle.lock` | shared: most `orx` commands (`command_uses_lifecycle_lock` in `src/main.rs`), `orx up`, the app, each 0.2.x supervisor; exclusive: `orx delete`, a storage migration | destroying or relocating shared state while anything runs |
| `.orx-remote-<hash>.lock`, the engine lock, beside the data directory or inside it when the parent is not writable (`src/commands/remote_host.rs`) | exclusive: each `orx up` for its whole life | one server owns a data directory's runs |
| `run-logs/<run>.supervisor.lock` and `.supervisor.shared-ssh.lock` (`src/engine/locks.rs`) | a 0.2.x supervisor, or the engine once it adopts the run | one owner per run |
| config `orx.data-dir-move.lock` | exclusive: a move; shared: file-deleting sweeps, the 0.2.x SSH service | a move starting under them |
| data `run-artifacts.lock` | shared: submission; exclusive: deletion, orphan sweeps | a run's files while it is submitted |
| data `submission-locks/<experiment>` | submission | one live run per experiment unless forced |
| data `orx.native-deletion.lock` | deleting chats with agent-native history | lookup, purge and row deletion across dashboards |

Narrower locks: `settings.lock` and `alphaxiv.lock` in config serialize settings writes and token refreshes, since each refresh retires the last token; `run-logs/<run>.cancel.lock` orders a run's cancel requests. Update locks belong to [desktop](desktop.md), host locks to [remote](remote.md).

**The lifecycle lock** is held for a command's whole life, and a headless `orx up` stays up while any run is unfinished, so it is held for as long as orx watches a job. It sits in a directory of its own so a sandboxed agent can be granted write access to that directory and nothing else in config. Its old path, `orx.lifecycle.lock`, is a hard link to the same file, so old and new binaries contend on one lock; orx refuses to run if the two stop being the same file, and keeps the old path alone on a filesystem that cannot link them.

Leases are rows in the store:

- `chat_turn_leases`: one live turn per chat, renewed by heartbeat, expired by a TTL.
- `chat_subscriptions` claims: one sweep delivering an update, expired by a TTL.
- `data_dir_move_lease`: a move in progress. New turns, orphan sweeps and deletions check it; the next process to find the move lock free clears a lease whose owner died.

## Moving the data directory

**A move never destroys the old tree before the new one is proven.** Only the dashboard moves it (`src/local/datadir.rs`), and only while nothing is active: it refuses under `ORX_DATA_DIR` or while any turn, run or project operation runs, then holds the move lock and lease so none starts. Idle agents stop first, because their homes under `agents/` hold live SQLite files ([agents](agents.md)).

The tree is renamed on the same filesystem, or copied and verified. Every absolute path recorded inside it is then rewritten (Git worktree links and remotes, project paths, OpenCode session directories; `src/local/storage.rs`), and the move fails if any reference to the old location survives. Only then does the persisted path swap. A failed rename renames back. A copy keeps the old tree and reports it instead of deleting it, because a detached supervisor or a request that resolved the old path just before the swap may still be writing there.

## Migrations follow paths

Old layouts are recognised by where things are, never by a marker recording that a migration ran (`src/local/storage.rs`). Repositories that older versions kept in the cache move into the data directory the first time a command finds them there, under the exclusive lifecycle lock; if other orx processes are running, the command stops and asks the user to close them, having moved nothing. A renamed folder is renamed in place when only the old name exists, and left alone when both exist.

Detection repeats on every start, so an interrupted rename or reference repair resumes on the next run. An interrupted cross-filesystem copy, or a repository root present in both places, stops with every file preserved and asks the user to resolve it.

## What leaves the machine

**Code, experiments, runs, logs, chats, transcripts and artifacts go to openresearch.sh only when the user publishes a project.** Otherwise its API serves accounts, organizations, SSH keys, sandboxes and compute catalogs, plus the telemetry and feedback below. A test keeps research-state endpoints other than publishing out of `src/client.rs`; `src/jobs/rental.rs`, the box's `orx-host` and `src/telemetry.rs` call the API beyond that test's reach. Other traffic:

- **Compute.** Work the user launches goes to the backend they chose as a source snapshot and the run's environment, on most backends all of `~/.openresearch/env` plus any configured Hugging Face token; logs and metrics come back. An OpenResearch box is provisioned through the API, and the snapshot reaches it over SSH. Compute settings check saved credentials against their providers.
- **The SSH public key.** Signing in uploads nothing. Only when the user adds a key, from the OpenResearch compute settings, `orx compute connect openresearch` or `orx ssh-key add`, does orx send a public key to their account: by default `~/.ssh/id_ed25519.pub`, creating the pair if it is missing. The private key never leaves the machine.
- **Remote network check.** Setting up Claude Code on a remote machine without an API key asks `ipaddress.to`, from that machine, for its public IP and hosting classification. Only the source IP reaches that service, and a failed lookup never blocks setup.
- **Paper search.** Queries and paper identifiers go to the public alphaXiv, OpenAlex, bioRxiv and PubMed APIs, without credentials, except an `OPENALEX_API_KEY` the user sets, which goes to OpenAlex. Paper PDFs come from arXiv or alphaXiv.
- **alphaXiv sign-in.** A browser trip to alphaXiv, whose identity token orx hands to openresearch.sh for its own token, or, with `orx login --alphaxiv`, to link the signed-in account. alphaXiv's tokens stay owner-only in the config directory and read the user's folders and papers. orx also reads the user's public profile, followed topics and researcher card: the dashboard shows the avatar alphaXiv names, and blank-project starter prompts are briefed from the profile, so it reaches the agent's model provider. The library picker loads thumbnails from alphaXiv's public asset CDN.
- **alphaXiv agent tools.** `orx alphaxiv-mcp` forwards research tool calls to alphaXiv, anonymously while signed out; signed in, it also reads library notes and highlights and, when requested, creates folders and saves papers. Tool arguments (research queries, paper and researcher identifiers, GitHub repository paths, folder identifiers and names) reach alphaXiv; results enter the agent's context and reach its model provider. The proxy sends no transcript, project files, experiments or logs of its own.
- **Telemetry.** Official builds send opt-out usage events to the first-party API under a random install ID that is not tied to an account: commands run; product events such as chats, messages, skills, experiments and retries, with the harness and coarse backend; harness detection and setup states; token usage per turn with model and provider labels; and a research profile, including the area and background text the user typed, when an older onboarding submits one. Source builds send nothing. Events queue in an outbox in the config directory and send in the background, so telemetry never blocks or fails a command. A change of the telemetry choice is itself recorded, so opt-outs are visible.
- **Feedback.** `orx feedback` sends an agent-written report to the first-party API: a summary, details, an optional quote, and context including the harness, chat session ID, versions, OS and install ID. The telemetry opt-out stops it. Signed in, the report also carries the account token, which ties that install ID to the account. `agent-skills/orx-feedback/` tells agents what to leave out.
- **Sharing.** Nothing leaves on its own: sharing writes a `.orx` file the user sends themselves (`src/local/share.rs`), holding a Git bundle of the baseline and experiment branches, the experiment tree, finished runs without their descriptors, their logs, the artifacts and the chats the user left ticked. A chat, log or artifact that looks like a credential refuses the share; the code is not scanned, since it is what the user chose to send. An opened file becomes a new project with no remote, and its runs carry the backend kind `imported`, which nothing launches, watches or cleans up.
- **Published projects.** Publishing, from the Share dialog or `orx project share --publish`, uploads exactly the file sharing would write, after the same scan, to openresearch.sh, which serves it to anyone with the link until the publisher deletes it. The upload names the local project ID, so publishing again replaces the file behind the same link.
- **Updates.** Each surface checks GitHub releases on its own schedule ([desktop](desktop.md)).
- **Installers the user starts.** Agent installers come from each vendor's site (`src/commands/up/harness_setup.rs`), Git for Windows from GitHub (`src/local/portable_git.rs`).
- **Integrations the user turns on.** Creating a project from a GitHub URL, which also asks GitHub's public API for its size; publishing to GitHub; syncing a paper with Overleaf, through its Git bridge or its live editor channel with the user's browser session cookie; Trackio metrics; and the agents' own model providers, which the agent talks to directly.
