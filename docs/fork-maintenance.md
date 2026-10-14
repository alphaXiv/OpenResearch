# Maintaining the research extensions

These additions deliberately use separate implementation files and small
registration changes. This reduces merge overlap; it cannot guarantee every
future upstream change remains compatible.

| Feature | Implementation owned by this fork | Upstream integration points |
| --- | --- | --- |
| Diagnostics | `src/commands/compute_doctor.rs`, `ui/src/features/research/ComputeDoctor.tsx` | Compute subcommand, settings mount, API route, Colab login probe |
| Measurements | `src/store/run_metrics.rs`, `src/commands/run_metrics.rs`, `ui/src/features/research/` | Module declarations, exp subcommand, experiments mount, API route |
| Local API | `src/commands/up/research_tools.rs` | Three routes in `up.rs`; existing workspace/auth handling |
| Deep research | `agent-skills/orx-deep-research/`, `src/local/deep_research.rs` | Two skill catalogs; scoped exceptions in delegation/literature skills |

No new dependencies, replacement harness framework or companion-service APIs
are required. UI strings follow the existing catalogs; compiled assets follow
the existing release workflow. Keep future changes within these boundaries
instead of reformatting shared files or duplicating backend implementations.

## Updating from upstream

1. Keep feature changes in separate, reviewable commits and start with a clean
   worktree. Back up the local database before running a new binary against it.
2. Inspect `git remote -v` and confirm which remote is the original upstream.
   Fetch it, inspect its changes, and merge its `main` into the fork's integration
   branch. Use the actual remote name; do not assume `origin` is upstream.
3. Resolve source registration and catalog conflicts semantically. Pay particular
   attention to `compute_settings::check`, Colab token/cache behavior, run/schema
   migrations, workspace query keys, and bundled skill resource installation.
4. Resolve UI source and lockfile conflicts first, then regenerate `ui/dist` with
   `pnpm build` in `ui/`. Do not hand-merge minified bundles or keep a whole side
   of a generated-assets conflict without rebuilding.
5. Follow `.github/workflows/ci.yml`. Focused regression entry points are
   `cargo test --locked --test research_tools_cli`, the `run_metrics` and
   `compute_doctor` Rust unit tests, and `ui/tests/researchComparison.test.mjs`.
   Also run the full existing checks, including bundled skill tests, because
   source registration changes can affect other harnesses.
6. Start a disposable instance through `scripts/dev-slot.mjs`, verify diagnostics
   and comparison there, rebuild the actual binary used day to day, then restart
   it. Check the doctor's executable path to avoid testing a stale installation.

Do not add blanket `merge=ours` rules, skip schema checks, or rewrite upstream's
release gates to suppress conflicts. If upstream introduces equivalent
functionality, migrate this extension's data and remove redundant hooks after
testing both old and new records. Prefer offering small generic fixes upstream
(such as CLI exit-status validation) separately from fork-specific research UI.
