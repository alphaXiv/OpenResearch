# OpenResearch (orx)

OpenResearch turns the coding agents a researcher already has (Claude Code, Codex, OpenCode, and more) into capable research agents. A researcher chats with an agent inside a project; the agent edits the code and runs each idea as an experiment: a commit, run on the laptop, a lab cluster or a cloud GPU, whose result comes back to the chat that asked. Logs, metrics and the figures and reports the agent writes stay on the researcher's own machines as the project's artifacts, each tied to the commit that produced it. We ship a desktop app for every platform, a web UI over `orx`, the CLI underneath it. Agents call `orx` directly to run the experiment loop, instructed by `./SKILL.md` and `./SYSTEM_PROMPT.md`.

| Path | Owns |
| --- | --- |
| `src/commands/` | One module per `orx` command; `up` is the server and its API, `app` the window. |
| `src/local/` | Projects and Git, chats (`chat/`), agent harnesses (`harness/`), skills, papers. |
| `src/engine/`, `crates/orx-host*` | The run engine in `orx up` and the `orx-host` binary that runs jobs on hosts. |
| `src/jobs/`, `src/compute.rs` | Backend clients, SSH config sharing, and the SSH service kept for 0.2.x runs. |
| `src/store.rs` | The SQLite store and its schema. |
| `src/client.rs`, `src/config.rs` | The openresearch.sh client, credentials, settings paths. |
| `src/updates*` | Self-update for the CLI and the desktop apps. |
| `ui/` | The dashboard: React, TanStack Router and Query, Paraglide messages. |
| `agent-skills/`, `agent-playbook/`, `SKILL.md`, `SYSTEM_PROMPT.md` | What orx tells agents, compiled into the binary; `node scripts/agent-surface.mjs` maps it. |
| `macos/`, `windows/`, `linux/`, `demo/` | Packaging and the onboarding demo. |

## Rules that matter most

- **The user's machine owns their research.** Code, experiments, runs, logs and chats never go to openresearch.sh unless the user publishes a project. Anything new that leaves the machine is a product decision: add it to the storage essay's list or don't ship it.
- **Every shipped version stays out there.** Old apps stay open for days, a remote host runs a different `orx` than the laptop, and openresearch.sh serves every client in the wild. The store, settings, run descriptors and wire protocols only grow: add fields, never rename or repurpose one. Compatibility code is for shipped contracts; an unmerged change has no users.
- **Loopback by default.** `orx up` binds 127.0.0.1 with no login, checks the Host of every request and the Origin of every request that changes state. Remote access goes over SSH; `orx up --host 0.0.0.0` is the user's explicit opt-in to a trusted private network.
- **Agents are the user's own tools.** orx runs them in home directories it prepares, reusing the user's sign-in, and never edits their agent configuration beyond the exceptions [agents](arc/agents.md#isolation) records.
- **Nothing is lost silently.** When orx cannot finish a turn, a queued message or a run, it records why and offers the way forward.

## Doing the task

- **Do the whole task.** Finish what was asked on every surface it reaches: a new backend or agent gets the setup flow and the logo its siblings have, and a feature that works in one view works wherever that thing appears. An issue does not bound the scope, and each review finding is fixed or gets its own issue. If the work should split, propose the split and let the user choose.
- **Fix the class, not the case.** When a bug is one instance of a pattern, fix it where every caller passes and check each site. A change that replaces a mechanism deletes the old one in the same change.
- **Raise the expensive choice first.** Before building on a choice that is costly to undo (how two views share one piece of UI, the shape of a command agents call, a file format or a stored field), name it to the user and get a yes.
- **Claim only what you saw.** Before saying something works, watch it work in the app or read it in the CI log, and say what you didn't check.
- **What works on one platform works on the others.** A menu item, shortcut, window control, drag or path built and seen on one OS has its counterpart on Windows, macOS and Linux, or the PR says why not.
- **Walk the failure paths.** For each step a feature takes, know what happens when it fails, when orx restarts in the middle, in a remote workspace, against an older peer, and while the CLI or an agent writes the same state. Each has an answer in the code or in the PR.

## Errors

- **Every failure reaches someone who can act on it.** One the user can fix becomes a message where they are looking ([orx-copy](.agents/skills/orx-copy/SKILL.md), "Errors"); one only a developer can fix is logged with its context. Discard an error (`let _ =`, `.ok()`, `unwrap_or_default()`, `.catch(() => {})`) only for best-effort work whose failure no one would notice: telemetry, a cache, a refresh that will run again.
- **Unknown is not absent.** Failing to read, parse, reach or inspect something is reported as that, never treated as missing, empty, stopped or signed out.
- **Each cause gets its own error,** raised where the cause is known and carrying the specific thing: the path, host, command or status (`.with_context(|| …)`). Never one generic message for several causes.
- **The API's message is the user's message.** `ApiError` sends `{"error": …}` and the UI shows it as is, so write it for the user; 4xx when the request or the user's setup is at fault, 5xx when orx is.
- **Recover only what is safe to repeat.** Retry an idempotent step with backoff and a bound; never retry an authentication failure ([remote](arc/remote.md#transport-and-the-auth-breaker)); never repeat a start that may have happened, inspect it instead ([experiments](arc/experiments.md)).
- **Agents read errors too.** A message `orx` prints or writes into a chat names the specific thing and the command that fixes it, since an agent may act on it without the user.
- **Test each cause a user can hit,** not only the success path.

## A run in one breath

A message reaches `orx up`, which starts or reuses the chat's agent in the chat's own worktree, with orx's playbook, skills and the `orx` CLI on its `PATH`. The agent commits and runs `orx exp run`: the dashboard archives the experiment's commit and submits it to a backend. The engine watches it and delivers results to the chat that asked. The page follows it through one event stream. See [the engine](arc/experiments.md#the-engine).

## Architecture

These documents introduce how the pieces of OpenResearch fit together: the reasons, invariants and boundaries a reader can't recover from the code. Exact behavior, numbers, flags, UI states and per-agent quirks live in the code and its comments. Read these before planning a change to a component. A change edits one only when it makes something the document says untrue, which is a deliberate architecture change the PR names; when an ordinary fix seems to need a doc edit, the detail belongs in a comment beside the code.

- [Where state lives](arc/storage.md): directories, the store, locks, egress.
- [Chats and agents](arc/agents.md): harnesses, isolation, permissions, recovery.
- [Experiments and compute](arc/experiments.md): snapshots, backends, the run engine.
- [Remote machines](arc/remote.md): the persistent host, `orx-host` and SSH transport.
- [The dashboard](arc/dashboard.md): the guard, live state, the React app.
- [The desktop app and releases](arc/desktop.md): app mode, updates, shipping.
- [Surviving restarts](arc/restarts.md): keepers, checkpoints, handing work to the next server.

## Running it

Run `pnpm --dir ui gallery` to open the standalone run strip gallery at `/gallery.html`.

`node dev.ts` runs a dev server per worktree, allocating ports for you.

```sh
node dev.ts [desktop] # start; `desktop` also opens the app window
node dev.ts status --json # URLs, readiness, attached terminals
node dev.ts logs api -n 100 # api, web, app or build output
node dev.ts restart # rebuild Rust and restart the backend
node dev.ts --state NAME # load a saved state; also --onboarding, --copy
node dev.ts save NAME | stop
```

It automatically coordinates with a user's running copy of it, so you can run it without asking. It starts from an empty state that has finished onboarding; `--onboarding`, `--state` and `--copy` start elsewhere. UI edits hot reload, Rust edits need the `restart` command. If `status --json` shows attached clients, don't stop it or overwrite the user's data.

Test a UI change the way a user meets it: from its entry point (a tile, a menu, an onboarding step) to the result, then the idle and empty states, both themes, a resize, Back and Escape. Use real agents where the change touches them; a cheap model is fine. A browser at the web URL is for iterating. The desktop app renders in WKWebView, WebKitGTK or WebView2, which break differently from Chromium, so check a visible change in the app window (`desktop`, macOS only; it puts a window on the user's screen) and on every other platform you can reach.

## Checks

CI runs what the changed files need: `cargo fmt --all`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`; in `ui/`, `pnpm typecheck`, `pnpm lint:i18n`, `pnpm lint:styles`, `pnpm test`; `node --test scripts/*.test.mjs`, `node --test 'src/**/*.test.mjs'` and `node --test scripts/dev.test.ts`; `node e2e/run.ts` when `src/` or `ui/` change ([e2e/README.md](e2e/README.md)). `cargo fmt` is the only formatter: never run another, such as Prettier, and never reformat lines you didn't change. Release builds embed `ui/dist`: run `pnpm build` in `ui/` before `cargo build --release`.

## Tests

- Test through a module's interface or the running app. Never parse, regex or evaluate source text (`createSourceFile`, `new Function`, YAML splitting); if logic is buried in a component, render the component or move the logic out.
- Fake only what orx doesn't own: agent CLIs, ssh, cloud APIs, openresearch.sh, the network. Never replace an orx function with a reimplementation.
- A test should fail when something a user would notice breaks, and keep passing when someone renames a function or rewords a message. Assert on what the user sees or what other code relies on; pin an exact string only when it is a contract, such as a CLI flag, a wire field or a stored value.
- Test each shipped contract both ways: new code reads what an old version wrote, and old code that rewrites a record keeps the fields a newer version added. Fixtures live in `tests/contracts/`.
- Tests never touch the user's machine. Under `cfg(test)` the data and config directories and every agent's native home live in a temporary root per process, a native home resolved outside it panics (`config::test_contained`; a launched `orx` gets the same homes from `tests/common/home.rs`), and a test that needs different environment variables runs in a child process (`config::in_test_child`). Never sleep on the clock; waits on background timers go through `config::paced`, which tests speed up. A test that needs a missing tool skips locally and fails in CI (`config::skip_without_tool`).
- UI tests render real React in happy-dom through `ui/tests/register.mjs` and `render.mjs`; `e2e/` drives the real `orx up` and dashboard with a fake agent.

## Code

- Fewer moving parts. Add a helper, field, type or abstraction only when it removes real duplication or encodes an invariant; inline one-caller helpers; derive values instead of storing copies; delete dead code on the way.
- Use what exists before adding: the platform API before a native bridge, `createDialogs` in `ui/src/dialog.tsx` before any other dialog, the primitives in `ui/src/components/ui/` before new ones, a sibling's flow before a new one. A new dependency, or a diff that only adds, needs its reason in the PR.
- UI work follows the skills in `.agents/skills/`: `orx-flows`, `orx-copy`, `orx-look`, `orx-testing`, `orx-accessibility`. `pnpm install` in `ui/` links them for Claude Code; a harness without skills reads them as files.
- UI text goes through Paraglide, with each new key in every catalog in `ui/messages/`. Style with theme tokens (`bg-background`, `text-subtext`), never raw colors.
- Default to no comment. Write one for a non-obvious why, an invariant the local code can't show, or a trap: one line, timeless, impersonal. History belongs in the commit. A comment about behavior that no longer exists is false; delete it with the behavior. Don't take this as an excuse to delete a comment that was already fine.

## Commits, issues and pull requests

Before you commit, open an issue or pull request, or comment on GitHub, it's required for you to follow the rules in [arc/contributing.md](arc/contributing.md). For example, you MUST NOT add `Co-authored-by` to your commits, instead following the rules in that document.

On the OpenResearch team (you can reach the ORX team in Linear), follow [Linear](arc/contributing.md#linear) before the first edit, without asking.
