# Where state lives

orx keeps a researcher's work on their own machine. Projects, experiments,
runs, chats and their transcripts never pass through a service, so the only
copy is the one on disk, and every orx process on the machine reads and writes
it at once: the dashboard, the desktop app, a terminal `orx`, an agent's `orx`,
a detached supervisor watching a job. This essay covers where those bytes
live, how the processes share them without corrupting each other, and the
short list of things that leave the machine.

## Three directories

```text
config   settings, credentials, locks, the telemetry outbox      stays put
data     the store, chat worktrees, run logs, agent homes,       one movable unit
         artifacts, attachments
cache    package caches, and the legacy repository root          rebuildable
```

**The data directory is one relocatable unit**, which the user can move from
the dashboard's storage settings. The few absolute paths recorded inside it
(Git worktree links, the paths of repositories orx cloned there, agent session
directories) are rewritten as part of a move, and the move fails, keeping the
old tree, if any reference to the old location survives. Its location resolves,
most to least authoritative, from an environment override (`ORX_DATA_DIR`),
the user's persisted choice in the config directory's settings, the XDG data
home, and finally `~/.local/share/openresearch`. The persisted choice is read
fresh on every resolution, so a process opening the store after a move sees
the new place. The directory is owner-only because the store holds scoped
compute capabilities.

**The config directory never moves**, which is why the settings that point
at the data directory, the credentials for openresearch.sh, alphaXiv and
Overleaf, and every cross-process lock live there. A lock inside the data
directory could not guard a move of that directory.

A desktop app launched from Finder inherits none of the user's shell
environment, so app mode imports a short list of variables from the login
shell at startup: `PATH`, the orx and XDG directory overrides, and the
variables that point each agent at its own configuration. Without them the app
and a terminal `orx` on the same machine would open different databases.

## The store

A SQLite database in WAL mode is orx's truth for everything that is not code:
projects, the experiment records, runs and their backend descriptors, chat
sessions, messages and turn leases, wake-up registrations, UI and workspace
state. Many short-lived processes open it concurrently, each with a busy
timeout; WAL lets readers proceed while one writer commits.

**The schema only grows.** Every open of the store applies the whole schema:
tables are created if missing, and each added column is an `ALTER TABLE ADD
COLUMN` whose failure on an existing column is ignored. Data fixes are written
to be idempotent and run every time. The rare backfill that is not idempotent
runs once in a single transaction, gated on SQLite's `user_version`, so a
half-applied one never records itself as done. Users run old and new orx
binaries against the same file, a dashboard left open for days beside a
freshly updated CLI, so:

- a new column needs a default an old binary can live with;
- a column is never renamed, retyped or given a new meaning;
- an old binary must be able to keep writing rows a new binary reads.

## Code lives in Git

A project is a Git repository the user owns, anywhere on disk; the store only
records its path. orx shells out to the user's `git` rather than embedding a
Git library, so every repository stays readable with ordinary tools. Each chat
works in its own worktree of the project, kept in the data directory under the
project and session, so agents never edit the user's checkout. What an
experiment is and how its branches relate belongs to
[experiments](experiments.md).

Per-project artifacts are a plain folder in the data directory. The
filesystem is their truth: there is no registry and no upload step, and the
dashboard only lists what is there, refusing any path that resolves outside
the folder.

## Run logs are files

A run's output is an append-only file named by the run, not rows in the
store. The supervisor appends as the job prints; the dashboard and `orx logs`
tail the file. Neither ever contends for a database write lock, and a chatty
job cannot bloat the database or slow every other writer.

## Agent homes

Each agent harness gets a configuration home inside the data directory. The
user's sign-in, settings, instructions, plugins and skills are linked in from
their own install, so the agent behaves as they configured it, while its
sessions and history stay in orx's home and never mix with the user's own.
Chats created before isolation existed keep resolving to the user's own
stores. Because these homes hold live SQLite files, a data-directory move
first shuts down every idle agent process. [Agents](agents.md) explains why
the homes exist.

## Locks

```text
lifecycle lock (config)   shared: nearly every orx command, `orx up`, the app,
                                  each detached supervisor
                          exclusive: `orx delete`, a one-time storage migration
dashboard lock (data)     shared: each dashboard; exclusive: a persistent remote host
move lock + move lease    exclusive: a data-directory move
turn and claim leases     rows in the store, with heartbeats and a TTL
```

**The lifecycle lock** is an advisory file lock in the config directory.
Ordinary commands hold it shared for their whole life, so the operations that
destroy or relocate shared state can take it exclusively and know nothing else
is running. A supervisor holds it too, which keeps `orx delete` out until
every watched job has finished or been detached. The lock sits in a directory
of its own so a sandboxed agent can be granted write access to exactly that
directory and nothing else in config. When the lock file's location changed,
the old and new paths became hard links to one file, so old and new binaries
contend on the same lock; orx refuses to run if the two paths stop being the
same file.

**A data-directory move** is non-destructive: validate the target, checkpoint
the database, copy the tree (or rename on the same filesystem), verify, swap
the persisted path, and only then delete the old copy. An interruption at any
point leaves the old tree whole and still authoritative. A move refuses to
start while any chat turn, run or project operation is active, and closes the
door behind it: it takes an exclusive file lock, then claims a lease row that
new turns and run claims check before starting. A lease whose owner died is
cleared by the next process that finds the file lock free.

## Migrations follow paths

Old layouts are recognised by where things are, never by a marker recording
that a migration ran. Repositories that older versions kept in the cache move
into the data directory the first time a command finds them there, under the
exclusive lifecycle lock; if other orx processes are running, the command
stops and asks the user to close them, having moved nothing. A renamed folder
is renamed in place when only the old name exists, and left alone when both
exist. Detection is cheap and repeats on every start, so a migration
interrupted halfway resumes on the next run.

## What leaves the machine

**Code, experiments, runs, logs, chats, transcripts and artifacts never go to
openresearch.sh.** The client for it is limited to accounts, organizations,
SSH keys, sandboxes and compute catalogs, and a test keeps research-state
endpoints out of it. Other traffic:

- **Compute.** Work the user launches goes, as a source snapshot, to the
  backend they chose; logs and metrics come back.
- **The SSH public key.** Signing in uploads nothing. Only when the user adds
  a key, from the OpenResearch compute settings,
  `orx compute connect openresearch` or `orx ssh-key add`, does orx send a
  public key to their account so OpenResearch boxes accept their SSH: by
  default `~/.ssh/id_ed25519.pub`, creating the pair if it is missing. The
  private key never leaves the machine.
- **Paper search.** Queries go to the public alphaXiv, OpenAlex, bioRxiv and
  PubMed APIs, without credentials.
- **alphaXiv.** Sign-in is a browser trip to alphaXiv, whose identity token
  orx hands to openresearch.sh for its own token, or, with
  `orx login --alphaxiv`, to link alphaXiv to the signed-in account. orx keeps
  alphaXiv's tokens owner-only in the config directory, refreshing them under
  a file lock since each refresh retires the last one, and reads the user's
  folders and papers with them. The chat's library picker loads paper
  thumbnails from alphaXiv's public asset CDN. Agents also send research tool
  calls to alphaXiv while signed out, limited by IP. Signed-in agents read
  library notes and highlights and, when requested, create folders and save
  papers to them. Tool arguments (including research queries, paper and
  researcher identifiers, GitHub repository paths, folder identifiers and
  names) go to alphaXiv; tool results enter the agent's context and reach its
  model provider. The MCP proxy sends no chat transcript, project files,
  experiments or logs of its own.
- **Telemetry.** Official builds send opt-out usage events to the first-party
  API under a random install ID that is not tied to an account: commands run,
  harness detection states, and the research profile chosen at onboarding.
  Source builds send nothing. Events wait in an outbox in the config directory
  and send in the background, so telemetry never blocks or fails a command. A
  change of the telemetry choice is itself recorded, so opt-outs are visible.
- **Updates.** Each surface checks for new releases on its own schedule
  ([desktop](desktop.md)).
- **Integrations the user turns on.** Pushing a project to GitHub, syncing a
  paper with Overleaf, and the agents' own model providers, which the agent
  talks to directly.
