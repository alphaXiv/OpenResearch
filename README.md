# OpenResearch CLI (`orx`)

> [!IMPORTANT]
> If you are an OpenResearch user or someone who is interested in autoresearch,
> we'd love to chat with you. Please email
> [contact@alphaxiv.org](mailto:contact@alphaxiv.org) if interested.

### Stay updated or use OpenResearch compute

To receive email updates about the OpenResearch CLI or spin up compute from
OpenResearch, create an account at [openresearch.sh](https://openresearch.sh).

### Run autoresearch on your machine

- **Run research agents in parallel**. Spins up agents in different worktrees
  so you can investigate several different directions at once.
- **Works with Claude Code, Codex, and OpenCode**
- **Bring your own compute**. Works with SSH, Slurm, Kubernetes, Modal,
  HuggingFace and more.
- **Give it a goal**. Can run the entire autoresearch loop from literature
  review to experiment analysis.
- **Local and private**. Your code and your data stays on your machine.

https://github.com/user-attachments/assets/33b62182-0795-490d-9366-0fb0b4bd49fd

## Quick start

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

The dashboard opens at `http://127.0.0.1:4791`. Give the agent a goal — for
example, ask it to reproduce a paper:

```
/reproduce-paper <paper URL or title> on <compute>
```

## The dashboard

`orx up` runs a single local process on `127.0.0.1` — an embedded web UI plus a
JSON/SSE API over a local SQLite store. From there you get:

- **Agent chat** — a research assistant with full project context, backed by
  your locally installed harness: Claude Code, Codex, or OpenCode (pick the
  harness and model in the UI). Ask it to analyze runs, dig into results, edit
  code, and spin up new experiments.
- **The experiment tree** — every experiment is a git branch: a runnable
  snapshot of your code. The root is your baseline; children are variants
  measured against it, so lineage stays explicit.
- **Runs** — every backend receives the same immutable archive of the recorded
  Git commit. Modal, Hugging Face Jobs, Kubernetes, Slurm, SSH, Ray,
  OpenResearch, and local runs do not require publishing the repository.
- **Autoresearch** — describe a goal and let the agent run autonomously toward
  it: proposing, launching, and analyzing experiments.

Everything binds to loopback only. Creating a local project and launching
compute do not publish code; upstream repositories and paper search use the
network only when you choose those import flows.

### On a remote machine

Develop from your laptop while the dashboard runs next to your GPUs:

```sh
orx up --remote user@host        # or an ~/.ssh/config alias; append :PORT for a custom SSH port
```

This starts `orx up` on the remote box over SSH, tunnels the port back, and
opens your browser locally. Note the remote server is unauthenticated on that
host's loopback, so other users on the same box can reach it.

## Commands

Run `orx --help` (or `orx <command> --help`) for full usage. The highlights:

| Area | Commands |
|---|---|
| Dashboard | `up` |
| Auth & organizations | `login`, `logout`, `orgs` |
| Projects | `projects`, `project` |
| Experiments | `create-experiment`, `exp status/run/cancel/wait/wake` |
| Runs & evidence | `runs`, `logs` |
| Compute | `compute`, `instance create` |
| Literature | `discover keyword/embedding/openalex/biorxiv`, `paper` (cross-corpus retrieval and paper reading — no login required) |
| Agent integration | `install-skills`, `skill` |
| Maintenance | `version`, `update`, `telemetry`, `delete database/cli/all` |

`orx install-skills` drops the OpenResearch skill into your local coding agents
(Claude Code, Codex, OpenCode, Cursor) so they can drive `orx` themselves —
`orx login` offers this too.

## Installing

The install script above fetches the latest prebuilt release (macOS and Linux,
x86_64 and arm64) and is the same as:

```sh
curl -LsSf https://github.com/alphaXiv/openresearch-cli/releases/latest/download/openresearch-cli-installer.sh | sh
```

### Staying up to date

Script-installed CLIs and the macOS app both update themselves. When a check
finds a newer release, a detached updater installs it in the background: the
CLI's *next* invocation is on the new version, and the app picks it up on its
next launch (the dashboard shows a "restart to finish updating" notice in the
meantime). Nothing blocks the command you actually ran, and a failing update
backs off rather than retrying on every invocation.

The macOS app verifies each download's checksum *and* requires a notarized
Developer ID signature from alphaXiv before replacing itself, so an unattended
swap can't install anything we didn't publish.

- `orx version --json` reports `channel` and whether `autoUpdate` is in effect.
- `orx update` updates now; `--dry-run` only reports.
- Settings → Updates shows the same state, with a toggle for automatic installs
  and, in the macOS app, **Install the `orx` command** — it links the app's
  binary onto your PATH so the CLI and the app are always the same build (also
  `orx install-cli`).
- Turn automatic installs off in Settings → Updates, or silence updates
  entirely with `ORX_NO_UPDATE_CHECK=1`.

Installs orx doesn't own — `cargo install`, Homebrew, Nix — are never modified.
They still get the outdated notice, and `orx update` then names the command to
run for that package manager.

### From source

Requires Rust (stable) via [rustup](https://rustup.rs). The prebuilt dashboard
UI is committed at `ui/dist`, so a plain build works:

```sh
cargo build --release          # binary at target/release/orx
cargo install --path .         # or install onto your PATH (~/.cargo/bin)
```

To hack on the dashboard UI itself (Vite + React, embedded into the binary at
build time):

```sh
cd ui && pnpm install && pnpm build
```

Run the tests with `cargo test`.

## Configuration

- **OpenResearch service URL** — used only for login, organizations, managed
  compute, sandboxes, and SSH keys. It defaults to production
  (`https://api.openresearch.sh`); override it with `--api-url` or
  `OPENRESEARCH_API_URL`.
- **Credentials** — `orx login` opens your browser, mints a personal access
  token, and stores it at `${XDG_CONFIG_HOME:-~/.config}/openresearch/credentials.json`
  (mode `0600`). Local projects, experiments, runs, logs, and files never use
  these credentials or sync to the service.

## Usage analytics

`orx` sends usage analytics linked to a random installation ID—not an account—to
help prioritize features. It's opt-out, and the `orx up` onboarding surfaces the
choice on first run.

- **Collected:** command name, a random per-install UUID, CLI version, OS/arch,
  the official build channel, a CI flag, coarse install type, and coarse event
  labels (e.g. onboarding completed, project created, chat session started, or
  a run launched on `modal`). For the onboarding research profile, only selected
  research-area categories and the number of linked papers are collected.
- **Not automatically added:** code, prompts, file contents or paths, project or
  experiment IDs/names, repo names, tokens, emails, or account identifiers.
  Research-profile free text, paper IDs, and paper titles are not sent. The
  random install UUID is not tied to your account.

```sh
orx telemetry off        # persistent, per-machine
orx telemetry status     # current state + the random install id
orx <cmd> --no-telemetry # per-run
```

Only official prebuilt release artifacts can send usage analytics. Source,
worktree, `cargo install --path`, and cargo-dist PR/dry-run builds remain off and
do not create an installation ID. `ORX_TELEMETRY_ENV=off` additionally disables
analytics in an official binary; it cannot enable analytics in a source build.

Events are fire-and-forget on a background task and never block a command.
They are sent to OpenResearch's first-party API and stored in Postgres.
