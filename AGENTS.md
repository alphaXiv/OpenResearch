# Repository Guide

## What this repository is

`openresearch-cli` is the open-source Rust implementation of the `orx` command-line tool. It owns the local OpenResearch experience: the CLI, the loopback dashboard and JSON/SSE API started by `orx up`, the local SQLite store, coding-agent integrations, experiment and run orchestration, and execution backends. The React/Vite UI in `ui/` is built into `ui/dist`; release builds embed those assets in the Rust binary, while debug builds read them from disk.

`~/openresearch.sh` is the companion hosted OpenResearch product. It owns the public website and documentation, the production API at `api.openresearch.sh`, accounts and organizations, cloud sandboxes, hosted agents, and OpenResearch-managed compute. This repository consumes that API for authentication and hosted services, while `openresearch.sh` also installs and runs `orx` on cloud agent boxes and proxies its daemon APIs.

The repositories are separate applications with a shared product and several cross-repository contracts. When changing production API routes or DTOs, authentication, OpenResearch compute, external-run mirroring, cloud-agent `orx serve` integration, installation behavior, or UI behavior intentionally shared with the hosted product, inspect the corresponding implementation in `~/openresearch.sh` and keep both sides compatible. Do not edit the companion repository unless it is explicitly in scope.

## Development guidelines

- For code-changing tasks, fetch `origin`, then start from current `origin/main` in a dedicated worktree, or reuse a worktree already dedicated to the current task. Read-only investigation can inspect `origin/main` directly. Do not modify the primary checkout merely to begin a task.
- Never run a development checkout against the user's live OpenResearch data or the default backend/UI ports. Use the repository dev-slot helper with an isolated database:

  ```sh
  node scripts/dev-slot.mjs start --worktree "$PWD" --db empty
  ```

  Use `--db copy` only when populated local data is explicitly required. Use the same helper's `status`, `stop`, and `cleanup` commands for that worktree.
- Rust code lives in `src/`; the dashboard lives in `ui/src/`. Keep local-only behavior local and use the production API client only for capabilities owned by `openresearch.sh`.
- `ui/dist` is committed and embedded in release builds. After changing UI source, run `pnpm build` from `ui/` and include the regenerated `ui/dist` assets in the change.
- In Tailwind code, prefer canonical utilities such as `flex flex-col h-full min-h-0`. Do not spell standard utilities as arbitrary properties such as `[display:flex]` or `[height:100%]`.
- Use the project's Tailwind theme aliases for design-system values, such as `bg-background`, `text-subtext`, and `border-border`. Do not bypass them with built-in palette colors or repeated CSS-variable arbitrary values.
- Arbitrary values are appropriate for genuinely custom values with no project utility, such as an exact calculated offset. Arbitrary properties are a last resort for CSS Tailwind does not support. Preserve semantic marker classes only when selectors or runtime behavior depend on them.
- Run verification in CI order before shipping:

  ```sh
  node --test scripts/dev-slot.test.mjs
  cargo fmt --all --check
  cargo clippy --all-targets -- -D warnings
  cargo build --locked
  cargo test --locked
  ```

  Also run `pnpm build` in `ui/` for UI changes.
