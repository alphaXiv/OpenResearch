# Repository Guide

## What this repository is

`openresearch-cli` is the open-source Rust implementation of the `orx` command-line tool. It owns the local CLI, dashboard and API, SQLite store, coding-agent integrations, experiment orchestration, and execution backends.

`openresearch.sh` is the companion service. It owns the website and documentation, accounts and organizations, sandbox provisioning, and managed-compute catalogs. Research projects, experiments, runs, logs, and artifacts remain local to `orx`.

When changing authentication, organization, sandbox, or managed-compute APIs, inspect the corresponding `openresearch.sh` implementation and keep both sides compatible. Do not edit the companion repository unless it is explicitly in scope.

## Development guidelines

- Rust code lives in `src/`; the dashboard lives in `ui/src/`. Keep local-only behavior local and use the production API client only for capabilities owned by `openresearch.sh`.
- Run local app instances through `scripts/dev-slot.mjs` so development data, ports, and processes stay isolated.
- `ui/dist` is committed and embedded in release builds. After UI changes, run `pnpm build` in `ui/` and include the regenerated assets.
- Prefer canonical Tailwind utilities (`flex flex-col h-full min-h-0`) and project theme aliases (`bg-background`, `text-subtext`, `border-border`). Use arbitrary values only when no project utility exists, and preserve semantic marker classes when selectors or runtime behavior depend on them.
- Before shipping, follow the checks in `.github/workflows/ci.yml`.

## CI and release gates

- GitHub protection for `main` must require the `fmt, clippy, test` and `version sanity` checks from GitHub Actions, including for administrators. Do not require a merge queue or require branches to be up to date. These settings are managed in GitHub, not by this file.
- PR CI must test GitHub's simulated merge (`refs/pull/<number>/merge`), which `actions/checkout` selects by default for `pull_request` events, rather than checking out the PR head alone. Each run tests its merge candidate; subsequent changes to `main` do not automatically rerun open PRs.
- CI also runs on `main`. Releases call the same CI workflow on the commit being packaged; publishing requires that run to succeed. Keep `./ci` in cargo-dist's `global-artifacts-jobs` when regenerating the release workflow.
