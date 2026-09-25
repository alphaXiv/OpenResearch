<div align="center">

<h1><img src=".github/readme-assets/openresearch.svg" alt="" width="36" /> OpenResearch</h1>

**The local-first workspace for research agents and autoresearch.**

<p>Turn <img src=".github/readme-assets/claude.svg" alt="" width="16" height="16" align="texttop" /> Claude Code,
<img src=".github/readme-assets/codex.svg" alt="" width="16" height="16" align="texttop" /> Codex,
<img src=".github/readme-assets/opencode.svg" alt="" width="16" height="16" align="texttop" /> OpenCode,
<img src=".github/readme-assets/cursor.svg" alt="" width="16" height="16" align="texttop" /> Cursor, Google Antigravity,
<img src=".github/readme-assets/kimi.svg" alt="" width="16" height="16" align="texttop" /> Kimi Code,
<img src=".github/readme-assets/minimax.svg" alt="" width="16" height="16" align="texttop" /> MiniMax Code, or
<img src=".github/readme-assets/zcode.svg" alt="" width="16" height="16" align="texttop" /> ZCode into research agents that can review
literature, develop hypotheses, run experiments, and produce research artifacts.</p>

<p>
<a href="https://github.com/artur-shaikhutdinov/OpenResearch-Kimi-MiniMax-ZCode/releases/latest/download/openresearch-cli-x86_64-pc-windows-msvc.zip"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/download-windows-dark.svg"><img src=".github/readme-assets/download-windows.svg" alt="Download OpenResearch for Windows (Beta)" width="220" height="44" /></picture></a>
<a href="#get-started"><picture><source media="(prefers-color-scheme: dark)" srcset=".github/readme-assets/install-linux-centered-dark.svg"><img src=".github/readme-assets/install-linux-centered.svg" alt="Install OpenResearch for Linux" width="220" height="44" /></picture></a>
</p>

<p>
<a href="https://openresearch.sh/docs"><img src=".github/readme-assets/action-documentation.svg" alt="Documentation" width="132" height="24" /></a><img src=".github/readme-assets/action-separator.svg" alt=" · " width="12" height="24" />
<a href="https://github.com/artur-shaikhutdinov/OpenResearch-Kimi-MiniMax-ZCode/releases"><img src=".github/readme-assets/action-releases.svg" alt="Releases" width="78" height="24" /></a>
</p>

<p><sub>Windows beta requires <a href="docs/windows.md">Git for Windows</a></sub></p>

<p><a href="https://trendshift.io/repositories/89363"><img src="https://trendshift.io/api/badge/repositories/89363" alt="GitHub Trending: #1 Repository of the Day" width="250" height="55" /></a>
<a href="https://trendshift.io/repositories/89363?utm_source=trendshift-badge&amp;utm_medium=badge&amp;utm_campaign=badge-trendshift-89363" target="_blank" rel="noopener noreferrer"><img src="https://trendshift.io/api/badge/trendshift/repositories/89363/daily?language=Rust" alt="alphaXiv/OpenResearch | Trendshift" width="250" height="55" /></a></p>

</div>

> **This is a fork** of [alphaXiv/OpenResearch](https://github.com/alphaXiv/OpenResearch)
> that adds [Kimi Code, MiniMax Code and ZCode](docs/new-harnesses.md) as agents.
> Its releases are Windows and Linux builds of the `orx` CLI, published from this
> repository, and `orx update` follows them. They are development-channel builds:
> no telemetry or feedback reaches alphaXiv, and there is no signed macOS app.
> Install on Windows (after [Git for Windows](docs/windows.md)):
>
> ```powershell
> powershell -ExecutionPolicy Bypass -c "irm https://github.com/artur-shaikhutdinov/OpenResearch-Kimi-MiniMax-ZCode/releases/latest/download/openresearch-cli-installer.ps1 | iex"
> ```
>
> On Linux:
>
> ```sh
> curl --proto '=https' --tlsv1.2 -LsSf https://github.com/artur-shaikhutdinov/OpenResearch-Kimi-MiniMax-ZCode/releases/latest/download/openresearch-cli-installer.sh | sh
> ```
>
> The rest of this README describes upstream OpenResearch.

## Get started

Install the CLI on macOS or Linux, then launch OpenResearch:

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up` opens the local dashboard at `http://127.0.0.1:4791`.

On a managed Mac (for example, a work computer), use the macOS download above
instead. Device-management policies may block the CLI that `install.sh`
installs because it is not yet signed. The app is signed with a Developer ID
and notarized by Apple. To use `orx` in your terminal, click **Install** under
**Install the `orx` command** in the app's Settings → Updates, or run (adjusting
the path if the app is not in `/Applications`):

```sh
/Applications/OpenResearch.app/Contents/MacOS/orx install-cli
```

Either way, `orx` is linked into `~/.local/bin`, with a hint to add it to your
`PATH` if needed. If you already ran `install.sh`, remove `~/.cargo/bin/orx`
first and open a new terminal.

On Windows, use the beta download above after installing
[Git for Windows](docs/windows.md).

[Connect a local model](docs/local-models.md) to use LM Studio, oMLX, Ollama,
or a custom endpoint with OpenCode.

Create an account at [openresearch.sh](https://openresearch.sh) to receive email
updates and use managed OpenResearch compute.

## Built for research agents

| | OpenResearch gives you |
|---|---|
| **Parallel exploration** | Give each research direction an independent agent session and isolated git worktree. |
| **Reproducible experiments** | Track variants in a git-native experiment tree; every run receives an immutable archive of its recorded commit. |
| **Evidence in context** | Keep logs, diffs, files, results, and artifacts tied to the work that produced them. |
| **Your choice of agent** | Use Claude Code, Codex, OpenCode, Cursor, Google Antigravity, Kimi Code, MiniMax Code, or ZCode, with the harness and model selected per session. |
| **Your choice of compute** | Run locally, on your own infrastructure, or with managed OpenResearch compute. |
| **Local ownership** | Keep projects, conversations, experiments, runs, logs, code, and artifacts on your machine. |

### Autoresearch

OpenResearch can run the full loop autonomously: propose an idea, change the
code, launch an experiment, inspect the evidence, and decide what to try next.
Multiple agents can explore different directions in parallel while the
experiment tree preserves their lineage.

## Run anywhere

The same committed source snapshot can run locally, over SSH, or on Slurm,
Kubernetes, Ray, Hugging Face Jobs, Modal, Tinker, and managed OpenResearch compute.
Publishing the repository is not required.

Run the workspace next to remote GPUs while using the browser on your laptop:

```sh
orx up --remote user@host
```

SSH config aliases and custom ports are supported. The remote service binds to
loopback and has no application-level authentication, so other users on that
host can reach it.

## CLI and agent integration

Install the OpenResearch skill into supported coding agents:

```sh
orx install-skills
```

Common commands:

```sh
orx projects
orx project view <project-id>
orx runs <project-id>
orx logs <run-id>
orx exp run <experiment-id>
orx discover keyword <query>
orx paper <arxiv-id-or-doi>
```

Run `orx --help` or `orx <command> --help` for the complete interface.

## Local by default

OpenResearch runs on `127.0.0.1` with a local SQLite store. Creating a project
or launching a run does not publish your code. An
[openresearch.sh](https://openresearch.sh) account is only used for
service-owned capabilities such as organizations and managed compute.

## Usage analytics

Official release builds send opt-out, coarse usage events tied to a random
installation ID. They do not include code, prompts, file contents or paths,
repository names, tokens, emails, or project and experiment identifiers.

```sh
orx telemetry off
orx telemetry status
orx <command> --no-telemetry
```

Source and development builds do not send analytics.

Coding agents may also file product feedback with `orx feedback` when you hit
a bug, wish for a feature, or get frustrated with OpenResearch. Each report is
a short description of the workflow problem, written to omit your research
details. Like analytics, reports are sent only from official release builds.
They are linked to your account when you are logged in and turned off by
`orx telemetry off`. The `--no-telemetry` flag covers only the command it is
passed to.
