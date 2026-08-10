# Repository Guide

## What this repository is

`openresearch-cli` is the open-source Rust implementation of the `orx` command-line tool. It owns the local CLI, dashboard and API, SQLite store, coding-agent integrations, experiment orchestration, and execution backends.

`openresearch.sh` is the companion hosted product. It owns the website and documentation, production API, accounts and organizations, cloud agents and sandboxes, and managed compute. The repositories are separate applications with shared product contracts: this repository consumes hosted APIs, while `openresearch.sh` installs and runs `orx` in hosted environments.

When changing APIs, authentication, managed compute, or hosted `orx` integration, inspect the corresponding `openresearch.sh` implementation and keep both sides compatible. Do not edit the companion repository unless it is explicitly in scope.

## Development guidelines

- Rust code lives in `src/`; the dashboard lives in `ui/src/`. Keep local-only behavior local and use the production API client only for capabilities owned by `openresearch.sh`.
- Run local app instances through `scripts/dev-slot.mjs` so development data, ports, and processes stay isolated.
- `ui/dist` is committed and embedded in release builds. After UI changes, run `pnpm build` in `ui/` and include the regenerated assets.
- Prefer canonical Tailwind utilities (`flex flex-col h-full min-h-0`) and project theme aliases (`bg-background`, `text-subtext`, `border-border`). Use arbitrary values only when no project utility exists, and preserve semantic marker classes when selectors or runtime behavior depend on them.
- Before shipping, follow the checks in `.github/workflows/ci.yml`.
