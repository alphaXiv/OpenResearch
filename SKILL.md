---
name: openresearch-cli
description: Use the `orx` CLI to run local OpenResearch projects from a terminal — create experiment branches, launch and supervise compute, inspect logs and evidence, and manage the experiment tree. Read this before driving `orx` programmatically.
---

# OpenResearch CLI (`orx`)

`orx up` owns projects, experiment branches, and execution in a local Git
repository and local database. Use the local session worktree to read, diff,
and edit code (see `orx-git`).

This overview is deliberately short: it carries the cardinal rules and a command
quick-reference, then points at focused **modules** for everything else. Load a
module with `orx skill <name>` (the bundled index is printed at the end of `orx
skill` output).

## Cardinal rules — read before doing anything else

These four govern everything below. Breaking any one silently invalidates your
results — they are not style preferences. The `orx-experiment-tree` module
expands on the why; these are the non-negotiables.

1. **Never edit a node once a run has answered it.** A node freezes the moment
   a run establishes its baseline or tests its hypothesis — that includes the
   root — and freezing is permanent: a disappointing result is still a result.
   Until then it is **provisional**: seeding it, fixing its deps, and making it
   run all happen on its own branch (`orx-experiment-tree`). To try an idea,
   branch a **child** and edit the child.
2. **The run command *and* the environment are a fixed contract — identical on
   every node.** A child inherits its parent's run command verbatim; leave it
   alone. Do **not** give nodes different start commands, and do **not** vary
   behavior through environment variables or env-prefixed commands
   (`LR=3e-4 python …`). The *only* thing that may differ between nodes is the
   **committed code/config** on the node's git branch. Set the local project's
   command once with `orx project edit <projectId> --run-command '<cmd>'`.
3. **Vary code, not knobs-in-the-command.** Encode hyperparameters in the
   code/config files and branch a child per variant — never sweep them by editing
   the run command or passing env vars. Every node runs the *same* command over
   *different code*, so their logged result summaries stay comparable.
4. **Grow the tree downward, not sideways.** Fan a little *within* a round (the
   options of one decision), then **descend onto that round's winner** for the
   next round. A root with a long row of direct children and no grandchildren is
   the failure mode. See "Shape the tree" in the `orx-experiment-tree` module.

If you're ever tempted to change the command, pass an env var, or pile another
node onto the root instead of branching a child, editing its branch, and
descending — stop. That's the anti-pattern, not a shortcut.

## Setup

```sh
orx login          # opens a browser, stores a token at ~/.config/openresearch/credentials.json
orx logout         # remove the stored token
```

- The API base URL resolves from `--api-url` → `OPENRESEARCH_API_URL` → a built-in
  default. Set `OPENRESEARCH_API_URL` for non-local use.
- Local project and run commands do not require a token. OpenResearch compute,
  instance provisioning, and account settings require `orx login`.

## Command quick-reference

Project-scoped commands take a **project id**; experiment-scoped commands take an
**experiment id**; run-scoped commands take a **run id**. Don't mix them — get
ids from `orx projects`, `orx project view`, and `orx runs` respectively. Each
group below has a module (`orx skill <name>`) with the full flags and rules.

### Auth
| Command | What it does |
|---|---|
| `orx login [--api-url <url>]` | Open a browser, do loopback OAuth, store a token. |
| `orx logout` | Remove the stored token. |

### Discover (project- and experiment-scoped)
| Command | What it does |
|---|---|
| `orx projects [--json]` | List projects in the local `orx` store. |
| `orx orgs [--json]` | List organization ids available for OpenResearch compute (login required). |
| `orx project view <projectId>` | Show a local project's details and experiment tree. **Experiment ids come from here.** |
| `orx runs <projectId> [--experiment <id>]` | List runs as a table, newest first. **Run ids come from here.** |

### Run evidence (run-scoped) — module `orx-evidence`
| Command | What it does |
|---|---|
| `orx logs <runId> [--head] [--bytes <n>] [--range <s>:<e>]` | Read a run's terminal log. |

### Create and run experiments (write) — modules `orx-create`, `orx-compute`, `orx-git`
| Command | What it does |
|---|---|
| `orx up` | Open the local dashboard to import or create a local project. |
| `orx project edit <localProjectId> [--name "<n>"] [--run-command "<cmd>"]` | Edit a local project's name or fixed run command. |
| `orx create-experiment <localProjectId> --title "<t>" [...]` | Add a local experiment node; prints its Git branch. |
| `orx compute [--gpu <id>] [--count <n>] [--provider <name>]` / `orx compute --cpu` | List the GPU/CPU compute catalog. |
| `orx instance create <orgId> (--gpu <id> … \| --cpu <flavor> …)` | Spin up a standalone instance in an org; see `orx-instances`. |
| `orx exp status/run/cancel/wait/wake <localExpId>` | Inspect, run, cancel, wait on, or register a wake-up for a local experiment node. |
| `orx exp desc <expId> [--set "<text>" \| --stdin]` | Read or overwrite the experiment's description. |
| `orx agent spawn "<task>" [--title "<t>"] [--stdin] [--no-wake]` | Delegate an independent task to a helper session; see `orx-agent-delegation`. |

To **read or edit** a node's code—including diffing what a run changed—use plain
Git in the local session worktree. See the `orx-git` module.

### Literature & papers — alphaXiv / OpenAlex / bioRxiv (no login required) — module `orx-lit-review`
Use before any web search for academic/research queries (paper, author, blog, model release).
| Command | What it does |
|---|---|
| `orx discover keyword "<query>"` | Call the alphaXiv full-text retrieval primitive with match snippets. |
| `orx discover embedding "<query>"` | Call the alphaXiv semantic retrieval primitive. The main agent ranks candidates and decides focused follow-ups; see `orx-lit-review`. |
| `orx discover openalex "<query>"` | Search the cross-disciplinary OpenAlex scholarly graph. |
| `orx discover biorxiv "<query>"` | Search bioRxiv preprints through OpenAlex's bioRxiv index. |
| `orx paper <id\|url> [--source ...] [--full]` | Fetch a paper: alphaXiv report with automatic full-text fallback (`--full` forces raw text), or OpenAlex/bioRxiv metadata+abstract. Source auto-detected from the id. |

### Meta
| Command | What it does |
|---|---|
| `orx skill [name[/resource]]` | Print this overview, one bundled module, or a lazily loaded module resource such as `compute/hf`. |

## Modules

The detail lives in focused modules — load one with `orx skill <name>` (the bundled
list, with one-line descriptions, is printed at the end of `orx skill` output):

- **orx-experiment-tree** — the experiment-tree model, the auto-research loop, and `orx exp desc`.
- **orx-create** — initialize a local project and add local experiment nodes.
- **orx-compute** — launch and monitor runs; after resolving the backend, read its bundled reference.
- **orx-instances** — create persistent standalone machines for manual work.
- **orx-git** — read, edit, and diff a node's code with plain git.
- **orx-agent-delegation** — delegate independent work to helper sessions safely.
- **orx-evidence** — capture and inspect experiment results through run logs.
- **orx-reports** — write durable research outputs into the project's artifacts directory.
- **orx-figures** — publication-quality figures in matplotlib or TikZ. Load it **before** writing any plotting code, then read the one reference for that figure type.
- **orx-paper** — draft a paper or preprint as LaTeX that renders and compiles to PDF.
- **orx-lit-review** — main-agent cross-corpus retrieval, source-selective follow-up policy, and paper content; the preferred starting point for academic/research queries.

## Typical workflow

Orienting in a project (read-only discovery):

```sh
orx projects                     # find the project id
orx project view <projectId>     # see the tree, pick an experiment id
orx skill experiment-tree        # the model + the auto-research loop
orx runs <projectId>             # find a run id
orx logs <runId>                 # read its output
```

To actually **drive** a project toward a goal — edit each node's code on its Git
branch and keep the round moving — follow the auto-research loop in
the `orx-experiment-tree` module. Every completed run is a decision point with
four moves: **repair** the same node when a run answered nothing, **refill**
the round with another sibling, **promote** the winner and descend, or **stop**.
