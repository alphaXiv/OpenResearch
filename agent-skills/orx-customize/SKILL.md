---
name: orx-customize
description: "Save reusable skills and LaTeX templates with `orx skills add` and `orx templates add`, and add or present agents and model providers with `orx agents`. Use when the user asks to create, import, or add a skill or template to OpenResearch for use across projects, or to add, name or fix how an agent, provider or model endpoint shows in OpenResearch."
---

# Customize OpenResearch

When the user asks to save a reusable skill or LaTeX template, create or prepare
its files in the working directory, then add them to OpenResearch:

| Command | Input |
|---|---|
| `orx skills add ./SKILL.md` | A skill with YAML frontmatter containing `name` and `description`. |
| `orx skills add ./research-workflow.zip` | A skill folder containing `SKILL.md` and its supporting files. |
| `orx templates add ./conference-style.tex` | A reusable LaTeX template. |
| `orx templates add ./conference-style.zip` | A template with its supporting `.cls`, `.sty`, and other files. |

Skill names come from frontmatter `name:`. Use lowercase letters, digits, and
single hyphens; built-in names and the `orx-` prefix are reserved. Template names
come from their filenames, so choose a descriptive filename.

These commands save across projects in the active OpenResearch data directory.
Same-name entries are fully replaced, including supporting files. Read the JSON
result and report the saved `name` and whether `replaced` is true.

Discovered native skills stay managed by the user's harness. These commands
create explicit OpenResearch uploads. Session copies refresh on subsequent
turns; do not promise an immediate skill reload in the running harness.

## Agents and model providers

OpenResearch lists every agent, account and model provider it can run,
including providers set up in the user's own OpenCode. When the user asks
to add a model endpoint, or a row shows the wrong name or logo, use these
commands from this chat; they act on the dashboard that started it.

| Command | Does |
|---|---|
| `orx agents` | Lists each row: its key, name, kind, whether it is on, and for providers the address, package and where it was found. `--json` for the full rows. |
| `orx agents label <key> --name "Team router" --logo openrouter` | Names a row and pins its logo. `--emoji 🧪 --color teal` sets its icon; `--clear-logo` goes back to the logo OpenResearch finds, and `--clear-name` to the name of an agent or provider found on this computer. |
| `orx agents add-endpoint --name "Tinker" --url https://…/v1 --model <id> --key-env TINKER_API_KEY --logo thinkingmachines` | Adds an OpenAI-compatible endpoint for OpenCode to run. The address is on this computer or HTTPS. `--key` passes the key itself; `--context` sets the model's context window. |

A logo is a brand's slug or name: `openai`, `claude`, `anthropic`, `gemini`,
`deepseek`, `openrouter`, `thinkingmachines`, `mistral`, `nvidia`,
`huggingface`, `together`, `groq`, `ollama` and most labs and hosts by name. One
the dashboard has no logo for falls back to the logo it finds itself. Before
adding, run `orx agents` to see whether the provider is already there: a
provider in the user's OpenCode config only needs a label. Ask the user before
adding an endpoint that bills their account, and never print a key.
