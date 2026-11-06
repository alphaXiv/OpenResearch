---
name: orx-testing
description: Test an OpenResearch change the way its user will find the problems, and bring evidence a reviewer can see. Covers starting from the right state, driving the real flow from its entry point, real agents and backends, states, themes, sizes, languages and failure cases, the desktop webviews on macOS, Windows and Linux, screenshots and video and checking each frame. Use before calling any change a user can see done, before filing or updating a pull request, when asked to test, verify, screenshot, record or QA, and whenever a user reports a bug that "the tests pass" on. In this repo, use it in place of any general UI testing skill, including when reviewing someone else's change.
---

# Testing OpenResearch like its user

## Start from the user's state

`node dev.ts --onboarding` for a first run, `--state NAME` for a saved one (build it up, then `node dev.ts save NAME`), `--copy` for realistic volume. AGENTS.md, "Running it", covers the rest of the dev server.

- **`--copy` brings the user's real projects and chats into frame.** Ask before evidence from it goes into a pull request, or build a state from scratch and save it.

## Walk the flow

1. **Enter where a user enters:** the tile, menu item, setup step or link that leads to the change, never a URL typed straight to the new page.
2. **Go all the way to the result:** the agent answers, the run finishes, the setting takes effect after a restart.
3. **Then use it naturally for a few minutes:** go Back, press Escape, resize, switch chats and machines, open it again. Most bugs a user finds come from this, after the change was called verified.
4. **Use real agents where the change touches them.** A cheap real model is fine (Haiku, a free OpenCode model, a local model). If an agent can't be signed in, say so and use a clearly labelled fixture; never present a fixture as a real run. The fake agent belongs to `e2e/`.
5. **A backend or agent change runs for real:** launch, follow the logs, cancel, and confirm the cleanup, on the cheapest tier of a real account. Delete what you created on every path, success or failure, and report the cost.
6. **Score it:** would a newcomer understand what to do on each screen without being told? Say where they wouldn't.

## The matrix

For each surface the change touches:

| Axis | Cover |
| --- | --- |
| States | first run, empty, loading, error, signed out, agent not ready, one backend down, offline (`OfflineBanner`), a remote workspace |
| Themes | light and dark, judged separately |
| Sizes | the default window, the narrowest it allows, fullscreen and maximized on each platform, long names and long lists |
| Language | English and one long locale (Russian or Polish, from Settings) for truncation and wrapping |
| Input | mouse, Tab and Shift-Tab, Enter and Escape, a double-click on submit, typing with an IME, paste |
| Timing | a slow response, a stream that drops mid-turn, a window that was hidden and comes back |
| Platforms | every platform the change can behave differently on (below) |

## The desktop webviews

WKWebView, WebKitGTK and WebView2 each break in their own ways (orx-look, "The desktop webviews").

- **Playwright's WebKit is the closest stand-in you can script** for WKWebView and WebKitGTK: `pnpm --dir e2e exec playwright install webkit`, then drive the web URL with `webkit.launch()`. It narrows the gap; it doesn't close it.
- **On macOS, `node dev.ts desktop` opens this worktree's app window.** Look at a visible change there before calling it done.
- **On Windows and Linux, test the window from a canary build in a VM you own,** never by taking over the user's desktop. WebView2 can be driven over CDP (`e2e/README.md`).
- **Updater, installer, window and path changes are tested on every platform they touch.**

## Evidence

- **Capture with your browser tool** at the web URL: its screenshots and recordings show the page without the rest of the screen.
- **Look at every frame before sending it,** for capture defects (devtools left open, a stray scrollbar, an offset, the wrong flow) and for product bugs you recorded without noticing. To check a video, extract its frames: `ffmpeg -i .shots/name.webm -vf fps=2 .shots/name-%03d.png`. `.shots/` is ignored.
- **Report as `arc/contributing.md`, "Evidence", asks,** with the score from step 6.
