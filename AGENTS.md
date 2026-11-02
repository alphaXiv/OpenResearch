# OpenResearch CLI

`orx` turns the coding agents a researcher already has (Claude Code, Codex,
OpenCode, Cursor, ACP agents) into research agents. One Rust binary is the
CLI, the `orx up` dashboard server and the desktop app; `ui/` is the React
dashboard it serves. Research state lives on the user's machine. The companion
service openresearch.sh owns accounts, organizations, sandboxes and compute
catalogs; when you change an API it serves, keep both sides compatible.

| Path | Owns |
| --- | --- |
| `src/commands/` | One module per `orx` command; `up` is the server and its API, `app` the window. |
| `src/local/` | Projects and Git, chats (`chat/`), agent harnesses (`harness/`), skills, papers. |
| `src/jobs/`, `src/compute.rs` | Compute backends and the run lifecycle `orx supervise` drives. |
| `src/store.rs` | The SQLite store and its schema. |
| `src/client.rs`, `src/config.rs` | The openresearch.sh client, credentials, settings paths. |
| `src/updates*` | Self-update for the CLI and the desktop apps. |
| `ui/` | The dashboard: React, TanStack Router and Query, Paraglide messages. |
| `agent-skills/`, `SKILL.md`, `SYSTEM_PROMPT.md` | What orx tells agents, compiled into the binary. |
| `macos/`, `windows/`, `linux/`, `demo/`, `docs/` | Packaging, the onboarding demo, user guides. |

## Rules that matter most

- **The user's machine owns their research.** Code, experiments, runs, logs and
  chats never go to openresearch.sh. Anything new that leaves the machine is a
  product decision: add it to the storage essay's list or don't ship it.
- **Every shipped version stays out there.** Old apps stay open for days, a
  remote host runs a different `orx` than the laptop, and openresearch.sh
  serves every client in the wild. The store, settings, run descriptors and
  wire protocols only grow: add fields, never rename or repurpose one.
  Compatibility code is for shipped contracts; an unmerged change has no users.
- **Loopback only.** `orx up` binds 127.0.0.1 without auth and checks the Host
  and Origin of every request that changes state. Remote access goes over SSH.
- **Agents are the user's own tools.** orx runs them in homes it prepares,
  reusing the user's sign-in, and never edits their agent configuration.
- **Nothing is lost silently.** When orx cannot finish a turn, a queued message
  or a run, it records why and offers the way forward.

## A run in one breath

A message reaches `orx up`, which starts or reuses the chat's agent in the
chat's own worktree, with orx's playbook, skills and the `orx` CLI on its
`PATH`. The agent commits and runs `orx exp run`: the dashboard archives the
experiment's commit, submits it to a backend, and a detached `orx supervise`
watches the job and tails its logs into the run. When it ends, the chat that
asked is woken with the result. The page follows it all through one event
stream.

## Architecture essays

Each explains a core system's theory and invariants, not its files. Read the
one for the area you touch, and fix any statement your change makes false.

- [Where state lives](arc/storage.md): directories, the store, locks, egress.
- [Chats and agents](arc/agents.md): harnesses, isolation, permissions, recovery.
- [Experiments and compute](arc/experiments.md): snapshots, backends, rentals.
- [Remote machines](arc/remote.md): the persistent host and the SSH service.
- [The dashboard](arc/dashboard.md): the guard, live state, the React app.
- [The desktop app and releases](arc/desktop.md): app mode, updates, shipping.

## Running it

`node dev.ts` runs one dev server per worktree, with its own ports, its data in
`.dev/` and a build of that worktree's code; never point it at the user's
installed data or running app. Without a terminal it starts the server if
needed, prints the URLs once ready and exits, leaving the server running.

```sh
node dev.ts [desktop]          # start; `desktop` also opens the app window
node dev.ts status --json      # URLs, readiness, attached terminals
node dev.ts logs api -n 100    # api, web, app or build output
node dev.ts restart            # rebuild Rust and restart the backend
node dev.ts --state NAME       # load a saved state; also --onboarding, --copy
node dev.ts save NAME | stop
```

Vite reloads UI edits; Rust edits need `restart`. If `status --json` shows
attached clients, someone is using the server: don't stop it or load a state
unless asked. Test UI changes in a browser at the web URL; `desktop` puts a
window on the user's screen, so open it only to test the app.

## Checks

CI runs what the changed files need: `cargo fmt --all`,
`cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`;
in `ui/`, `pnpm typecheck`, `pnpm lint:i18n`, `pnpm lint:styles`, `pnpm test`;
`node --test scripts/*.test.mjs` and
`node --test --experimental-strip-types scripts/dev.test.ts`. Release builds
embed `ui/dist`: run `pnpm build` in `ui/` before `cargo build --release`.

Other scripts: `build-macos-app.sh` and `package-macos-app.sh` sign and
notarize the app and DMG (`macos/DISTRIBUTION.md`), `build-linux-appimage.sh`
builds the AppImage, `generate-*` draw brand assets and the demo's evidence,
and `ci-*`, `canary-*`, `find-release-run.sh` and `release_notes.py` serve the
workflows.

## Code

- Fewer moving parts. Add a helper, field, type or abstraction only when it
  removes real duplication or encodes an invariant; inline one-caller helpers;
  derive values instead of storing copies; delete dead code on the way.
- UI text goes through Paraglide, with each new key in every catalog in
  `ui/messages/`. Style with theme tokens (`bg-background`, `text-subtext`),
  never raw colors.
- Default to no comment. Write one for a non-obvious why, an invariant the
  local code can't show, or a trap: one line, timeless, impersonal. History
  belongs in the commit. A comment about behavior that no longer exists is
  false; delete it with the behavior.

## Commits and pull requests

- Title with a type, optionally scoped: `feat:`, `fix(ui):`, `chore:`, `ci:`.
- Credit each model with `Assisted-by: claude-opus-5.5` (or `gpt-6.1-sol`,
  one trailer per model). Never add `Co-authored-by` for a model.
- Say why and what is surprising; the reviewer reads the diff, so don't list
  hunks. Pull requests from forks must link an issue with `Closes #N`.
