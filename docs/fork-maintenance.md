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

## Where the fork touches upstream files

Fork code lives in its own files. Upstream files only carry a small hook that
mounts it, so a sync should only conflict at these lines. List the current
touch points with:

```sh
git fetch upstream main
git diff --numstat upstream/main -- . ':!ui/dist' ':!ui/messages' \
  | while read a d f; do git cat-file -e "upstream/main:$f" 2>/dev/null && echo "$((a+d)) $f"; done | sort -rn
```

| Fork code | Hook in the upstream file |
| --- | --- |
| `src/commands/up/fork_settings.rs` (all fork API routes: research tools, harness accounts, decision model, cross-harness, compute routing) and `fork_compute.rs` (prices, Colab, GCP, Kaggle handlers) | `mod` lines and one `.merge(fork_settings::routes())` in `up.rs`; one `use super::fork_compute::{...}` in `compute_settings.rs` for `orx compute show/configure` |
| `src/commands/compute/prices.rs` (`orx compute prices`) | `mod prices;` and the `Prices` subcommand variant in `compute.rs` |
| `src/commands/supervise/gcp_run.rs`, `memory_guard.rs`, `colab_resume.rs` | `mod` lines, the `gcp_job` dispatch, the memory guard calls and the Colab resume check in `run_local` in `supervise.rs` |
| `src/jobs/modal/credit.rs` (Modal credit and list prices) | `mod credit; pub use credit::*;` and the `billing` launcher subcommand in `modal.rs` |
| `ui/src/features/fork/ComputeSections.tsx` (Colab, Kaggle cards), `SettingsCards.tsx` (harness accounts, failover, decision model, cross-harness, compute routing, provider prices, GCP) | two imports and the card mounts in `SettingsPage.tsx`; the shared class names and helpers they use are exported from `SettingsPage.tsx` |
| `src/commands/discover/extra_sources.rs` (`orx discover arxiv\|semanticscholar`) | `pub(crate) mod extra_sources;` and two match arms in `discover.rs`; two `DiscoverCommand` variants in `main.rs`; `pub(crate)` on `http`, `public_get`, `compare_dates` in `client.rs`; the filter in `set_lit_sources_settings` in `up.rs` that keeps fork toggles; `/api/settings/fork-lit-sources` lives in `fork_settings.rs` |
| `ui/src/features/fork/litSourceMeta.ts`, `ForkLitSourcesList.tsx` (arXiv and Semantic Scholar names, marks, toggles) | `ForkLitSource` in the `LitSource` union and strategy check in `orxCommand.ts`; two spreads in `LitSourceLogo.tsx`; `<ForkLitSourcesList />` in `LitSourcesPicker.tsx`; two activity labels in `ChatPanel.tsx` |
| `ui/src/features/fork/computeApi.ts` (Colab, GCP, Kaggle types and calls) | `export * from "./features/fork/computeApi";` in `api.ts` |
| `src/jobs/{colab,colab_account,colab_runtimes,gcp,gcp_billing,kaggle,provider_prices}.rs`, `src/local/gcp.rs` | `mod` lines and backend dispatch in `jobs/mod.rs`, `compute.rs`, `localrun.rs` |
| `src/local/harness/{accounts,failover}.rs`, `src/store/harness_accounts.rs`, `src/commands/up/harness_accounts.rs` | Turn routing in `local/chat/mod.rs`, account homes in `native_store.rs`, harness CLI in `commands/agent.rs` |

Still inline: the harness, routing and decision model types in `api.ts`, the
chat-time account routing in `local/chat/mod.rs`, and `recommended_model` in
`up.rs`. These are woven into upstream logic rather than standalone blocks.

UI strings must stay in `ui/messages/*.json` because `check-i18n` reads only
those catalogs. Add new keys next to related fork keys rather than at the end of
the file, and resolve catalog conflicts by keeping both sides.

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
