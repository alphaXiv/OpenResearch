---
name: orx-customize
description: "Save reusable skills and LaTeX templates with `orx skills add` and `orx templates add`. Use when the user asks to create, import, or add a skill or template to OpenResearch for use across projects."
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
