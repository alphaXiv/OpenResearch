# The desktop app and releases

The desktop app is the same `orx` binary the CLI is. One executable, built
once per target, is the command-line tool an agent shells out to, the `orx
up` server, and a windowed app that wraps that server. Shipping one binary
means one version to reason about and one thing to update. It also means the
binary has to decide, from how it was started, which of those it is, and the
CLI an agent calls must never open a window.

## One binary, several faces

```text
macOS    OpenResearch.app/Contents/MacOS/OpenResearch, no arguments ─► app
         the bundle's `orx` symlink, or any arguments               ─► CLI
Windows  OpenResearch.exe (launcher) ─► `orx.exe app [link]`        ─► app
         orx.exe double-clicked from Explorer                       ─► relaunch as app
         orx.exe in a terminal                                      ─► CLI
Linux    the AppImage's entry script ─► `orx app [link]`            ─► app
         `orx`                                                      ─► CLI
```

- **macOS** enters app mode only when the bundle's executable is started
  under its own name with no arguments, which is how Finder starts it. The
  bundle also carries an `orx` symlink to the same file. The check compares
  the name the process was invoked as with the canonical executable, so the
  symlink stays a plain CLI and an agent running `orx` gets help, not a second
  dashboard.
- **Windows** gives a GUI program no console, and every console program it
  starts would flash a window of its own. The installed launcher therefore
  starts `orx.exe app` in a hidden console that orx and every Git, shell and
  agent process it runs share.
- **Linux** runs the AppImage's entry script, which points GTK and WebKit at
  the bundled libraries, records the session's own values so the programs
  orx opens get them back, and starts `orx app`. The app keeps a desktop
  entry installed so it appears in the launcher and handles `openresearch:`
  links.

App mode takes only an `openresearch:` link as an argument; any other
argument list is a CLI invocation.

## App mode

The main thread owns the window's run loop, as every desktop toolkit requires,
while the dashboard server runs on the async runtime's worker threads. A
single-threaded runtime would deadlock here.

- **A fixed port.** The window's local storage is keyed by origin, so the app
  binds the same port every time. If a terminal `orx up` or another install
  holds it, the app falls back to a free port. After an update relaunch it
  keeps the port it had.
- **One app per user.** macOS enforces this itself. On Windows a named event
  and on Linux a socket in the user's private runtime directory let a second
  launch bring the running window forward, handing it any link it was opened
  with, and exit.
- **The user's shell environment.** An app started by launchd or a desktop
  session sees a bare `PATH` and none of the user's exports. At startup the
  app asks the user's interactive login shell for a short list of variables
  (the `PATH`, the orx and XDG directory overrides, each agent's config
  location) and uses those. Otherwise it would miss agents installed through
  the shell and open a different database than a terminal `orx`
  ([storage](storage.md)).
- **The same locks as `orx up`.** The app takes the lifecycle lock before
  anything else, so `orx delete` from a terminal cannot remove the store from
  under a running app.

## The window and the page

The page talks to the window through a small message bridge. It may ask for:
zoom in, out and reset, persisted per user; dragging and maximizing from a
titlebar the page draws; the window's appearance following the page's theme;
on Windows, custom window controls; and on macOS, a native menu bar whose
structure and state the page sends. The window never runs commands on the
page's behalf.

What leaves the window:

- A pop-up never opens inside the app. `http:`, `https:` and `mailto:` links
  go to the system browser; any other scheme is dropped, because the browser
  would ask before launching another app and a direct open would not.
- A download asks where to save, as a browser does.
- `openresearch:` links, from the system or a second launch, become in-page
  navigation, so the page's guards against losing unsaved work still apply.

## Updates

Every surface learns about releases the same way: it fetches a small manifest
through GitHub's latest-release download link, a CDN redirect with no API rate
limit, and caches the answer for a day. How the update is applied depends on
how this copy of orx was installed.

| Installed by | Applies its own updates by |
| --- | --- |
| the shell or PowerShell installer | running the installer again; a receipt proves the binary is the installer's |
| the macOS app | downloading the DMG, checking its hash, signature and notarization, renaming the new bundle into place |
| the AppImage | downloading the new file, checking its hash, renaming it over the old one |
| a Windows portable copy | staging the new `orx.exe`, then two renames |
| cargo, Homebrew, Nix, anything else | never; writing over a package manager's files would break it |

**Replacement is always a rename.** A running process keeps its old file and
code signature until it restarts; nothing is ever half-written underneath it.

**An update never swaps the binary under a running `orx up`.** Each server
holds a shared lock on its install for its whole life, and an updater needs
that lock exclusively, because agents would otherwise get a different `orx`
than the server running them. If a server is up, the update is recorded as
deferred. The server relaunches into it when idle ([dashboard](dashboard.md)),
and the relaunched process waits for its predecessor to release the port.

Background updates run as a detached process and back off after repeated
failures. The CLI prints a one-line warning when a newer release exists.

**Channels.** The desktop apps can follow stable releases (`v1.2.3`) or canary
builds (`canary-YYYYMMDD.N`), one per merge to main. Switching channels is
refused while a downloaded update waits for a restart.

## The build channel

A build is **production** only when it is an official release built in the
project's own GitHub Actions; every other build, including every local one,
is **development**. The build script refuses to mark a build official
anywhere else. Production builds may send telemetry and may install released
binaries on remote machines. Development builds do neither, and a locally
built AppImage or Windows app never replaces itself with a release. Canary
builds also embed the exact source commit they were built from, checked
against the workflow's own commit.

## How a version ships

**The release act is merging a reviewed pull request that bumps the version
in the manifest.** A push to main that changes the version dispatches the
release workflow; nothing pushes tags, and a version never moves without a
human decision. A version check on every pull request requires the version
to move forward, to never reuse a released tag, and to come with a
regenerated lockfile.

```text
merge bump ─► release workflow ─► build and sign every target
                                ─► CI on the exact packaged commit
                                ─► verify the build channel and telemetry contract
                                ─► publish the release (creates the tag)
           ─► desktop workflows ─► DMG (signed, notarized), Windows installer,
                                   AppImages; update manifests; download links
merge any  ─► canary workflow  ─► CI on that commit ─► canary desktop builds
```

- **Nothing publishes untested code.** The release workflow calls the same CI
  workflow on the commit being packaged, and publishing requires it, the
  build-channel check and the telemetry-contract check to pass. CI stays in
  the release tool's global-artifacts jobs for this reason.
- **Release notes** are GitHub's generated notes, preceded by download buttons
  that link only assets actually uploaded, refreshed as each desktop package
  attaches, plus the issues the release's pull requests closed.

## The gate on main

Main accepts a merge only when three checks pass, administrators included:
"fmt, clippy, test", "version sanity" and "linked issue". There is no merge
queue and no requirement to be up to date with main.

- Pull request CI tests GitHub's simulated merge of the branch into main.
  It selects jobs by which files changed. The required aggregate check fails
  on any failed job and on any job skipped that the changed files should
  have run.
- Pull requests run a lighter platform matrix. Full platform tests, optimized
  builds and packaging run only after merge.
- "Linked issue" requires pull requests from forks to name an issue they
  close; branches in the repository are exempt. It runs from main's copy of
  the workflow with the target repository's permissions, so it **must never
  check out or run the pull request's code**.
