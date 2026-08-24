<div align="center">

# OpenResearch

**The local-first workspace for research agents and autoresearch.**

Turn Claude Code, Codex, or OpenCode into research agents that can review
literature, develop hypotheses, run experiments, and produce research artifacts.

[Download the desktop app](https://openresearch.sh/download) ·
[Documentation](https://openresearch.sh/docs) ·
[Releases](https://github.com/alphaXiv/openresearch-cli/releases)

</div>

## Get started

Download the local desktop app from
[openresearch.sh/download](https://openresearch.sh/download), or install the
CLI on macOS or Linux:

```sh
curl -LsSf https://openresearch.sh/install.sh | sh
orx up
```

`orx up` opens the local dashboard at `http://127.0.0.1:4791`.

Create an account at [openresearch.sh](https://openresearch.sh) to receive email
updates and use managed OpenResearch compute.

## Built for research agents

| | OpenResearch gives you |
|---|---|
| **Parallel exploration** | Give each research direction an independent agent session and isolated git worktree. |
| **Reproducible experiments** | Track variants in a git-native experiment tree; every run receives an immutable archive of its recorded commit. |
| **Evidence in context** | Keep logs, diffs, files, results, and artifacts tied to the work that produced them. |
| **Your choice of agent** | Use Claude Code, Codex, or OpenCode, with the harness and model selected per session. |
| **Your choice of compute** | Run locally, on your own infrastructure, or with managed OpenResearch compute. |
| **Local ownership** | Keep projects, conversations, experiments, runs, logs, code, and artifacts on your machine. |

### Autoresearch

OpenResearch can run the full loop autonomously: propose an idea, change the
code, launch an experiment, inspect the evidence, and decide what to try next.
Multiple agents can explore different directions in parallel while the
experiment tree preserves their lineage.

## Run anywhere

The same committed source snapshot can run locally, over SSH, or on Slurm,
Kubernetes, Ray, Hugging Face Jobs, Modal, and managed OpenResearch compute.
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
