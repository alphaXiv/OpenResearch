# End-to-end tests

Playwright drives the real dashboard against the real `orx up`: the Rust server, its SQLite
store, Git worktrees, the chat host, `orx exp run` and its detached supervisor. Only what orx
does not own is faked.

```text
Playwright worker ─┬─ browser ──────────────► orx up (this checkout, debug build, ui/dist)
                   │                           HOME, XDG and TMPDIR in /tmp/orx-e2e-*
                   │                           PATH=/usr/bin:/bin:/usr/sbin:/sbin
                   ├─ agent scripts ◄── turn ── fake ACP agent (src/fake-agent.ts), which
                   │   (src/agent.ts)   holds    orx spawns as it spawns any ACP agent
                   └─ outside ◄──────────────── OPENRESEARCH_API_URL, and HTTP(S)_PROXY for
                       (src/outside.ts)          everything else, refused and recorded
```

```sh
node e2e/run.ts                       # build orx and the dashboard, then run every spec
node e2e/run.ts chat.spec.ts --headed # arguments pass through to Playwright
pnpm --dir e2e exec playwright install chromium   # once, and after a Playwright upgrade
pnpm --dir e2e report                 # the last run's traces and logs
ORX_E2E_KEEP=1 node e2e/run.ts        # keep each server's /tmp/orx-e2e-* sandbox
```

Each test's report carries `browser.log`, `orx.log`, `agent.log` (every ACP message) and
`outside.log`. A failed test keeps its own server's sandbox.

## Writing a spec

Import `test`, `expect`, `send` and `composer` from `src/test.ts`.

- **One server per worker, one project per test.** The `project` fixture gives the test a
  blank project on the worker's onboarded server; tests share nothing else. A test that needs
  onboarding or a restart takes a server of its own from `launchOrx()`.
- **Script the agent before the prompt.** `agent.script(match, steps)` plays `steps` for the
  next prompt containing `match`: `say`, `think`, `run` (a shell command through orx's terminal),
  `hold` and `exit`. A test fails if a script went unused or a prompt matched none.
- **Hold instead of timing.** `await agent.reached(name)` waits until the turn stops at
  `{ hold: name }`; `agent.release(name)` lets it go on. Assert what the page shows in between.
- **Arrange through the API, act in the browser.** `orx.onboard()` and `orx.createProject()`
  set up what the test is not about, through the routes the dashboard calls; the journey itself
  clicks and types.
- **Nothing leaves the machine unlisted.** A test fails on any request off the machine that
  `listedEgress` in `src/outside.ts` does not name. Adding one is a product decision: list it in
  [arc/storage.md](../arc/storage.md) first.

`lint.ts` rejects the ways a spec hides a race, as alphaXiv's `no-e2e-race-hiding` does: no
`waitForTimeout` or sleeps, no `toPass` retries, no `networkidle`, no `force: true`, no serial
mode, no unconditional skip. Wait for state; when the product itself waits on a timer, give that
one assertion a timeout and say which timer.

## Why not the desktop window

The desktop app is a webview over this same dashboard and server, so the browser covers all but
the window itself: the native menu, zoom, window controls and link routing. Of the three
webviews only Windows' can be driven without changing the app: WebView2 reads
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`, and Playwright attaches
with `chromium.connectOverCDP`. WKWebView on macOS has no WebDriver for third-party apps, and
WebKitGTK on Linux needs the app to opt into automation before `WebKitWebDriver` can attach.
