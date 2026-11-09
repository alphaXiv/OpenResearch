# The desktop app and releases

Each platform's package wraps one `orx` binary that is the CLI an agent shells out to, the `orx up` server, and a window around that server: one version to reason about, one thing to update. The binary therefore decides what it is from how it was started, and the CLI an agent calls must never open a window. App mode lives in `src/commands/app.rs`, under the `desktop_app` cfg (`build.rs`): Linux's static musl CLI archives have no window; only the AppImage's glibc build does.

## One binary, several faces

| Platform | Started as | Becomes |
| --- | --- | --- |
| macOS | the bundle's executable under its own name, no arguments | app |
| macOS | the bundle's `orx` symlink (an agent gets help, not a dashboard), or any arguments | CLI |
| Windows | `OpenResearch.exe`, which starts `orx.exe app [link]` | app |
| Windows | `orx.exe` double-clicked | relaunches as `orx.exe app` |
| Linux | the AppImage, whose `linux/AppRun` starts `orx app [link]` | app |
| Windows, Linux | `orx` in a terminal | CLI |

Anything but `app`, `app <openresearch: link>` or `app <file.orx>` is a CLI invocation. A file becomes a `file:` link, so both travel one path, including to a running app.

- **Windows** would flash a window for every console program a GUI process starts, so `windows/launcher/` starts `orx.exe app` in a hidden console shared by orx and everything it runs. **The launcher changes only with a new installer**, so `orx app [link]` is a contract with every installed launcher. `windows/OpenResearch.iss` installs per user, so `orx.exe` can replace itself without elevation.
- **Linux** bundles WebKitGTK in the AppImage, so its library floor is glibc's, set for every user by the Ubuntu runners that build it. AppRun records the session's values before running the GTK hooks once, points GTK at the bundle and starts orx inside the read-only mount; orx restores those values for the host programs it opens (`src/local/shell_env.rs`, in step with AppRun's list). The session chooses Wayland or X11 unless the user sets `GDK_BACKEND`; the bundling plugin's X11 default must not override that choice. Each start points a desktop entry at the AppImage's current path, which also claims `openresearch:` links and `.orx` files.

## App mode

The main thread owns the window's run loop, as every desktop toolkit requires, while the server runs on the async runtime's workers; a single-threaded runtime would deadlock.

- **A fixed port.** The window's local storage is keyed by origin, so the app binds the same port every time, falls back to a free one if something holds it, and keeps its port across an update relaunch.
- **One server per data directory.** If an `orx up` already owns the data directory, the window opens on it instead of starting a second engine.
- **One app per user.** macOS enforces this itself; on Windows and Linux a second launch signals the first, which comes forward and takes any link.
- **The lifecycle lock.** App mode takes it itself, so `orx delete` from a terminal cannot remove the store under a running app ([storage](storage.md)).
- **The user's shell environment.** On macOS and Linux an app started by launchd or a desktop session sees a bare `PATH` and none of the user's exports, so it would miss shell-installed agents and open a different database than a terminal `orx`. Before touching storage it asks the interactive login shell for a short list of variables (`src/local/shell_env.rs`). A late answer supplies `PATH` alone, because switching directories in use would split state. Every child is built in `src/process.rs`, so it gets those values, and "installed" on the dashboard and "found" at spawn cannot disagree.

## Closing and quitting

**Closing the window never stops the engine's work.** When the app runs its own server:

- **Close while busy:** the window hides, the page unloads and the engine runs windowless. Busy means an unfinished run, an undelivered notice, a running chat turn or an open dashboard event stream. The window's own page holds such a stream until it unloads, so a close with a local workspace open goes windowless even with no work.
- **Close while not busy:** the app quits.
- **Windowless and idle:** the app quits after the idle wait of a headless `orx up` ([experiments](experiments.md)).
- **Windowless, then a second launch, a link or a macOS Dock reopen:** the window returns on a fresh page load.
- **Explicit quit, which only macOS has:** with unfinished runs it asks first, and they continue on their hosts for the next start to collect. System logout and shutdown never ask.

A window opened on another process's server just closes.

## The window and the page

The page reaches the window through a small message bridge: zoom; on macOS and Windows, a titlebar the page draws; on macOS, the appearance and a native menu the page specifies. The window never runs commands on the page's behalf.

- `http:`, `https:` and `mailto:` pop-ups go to the system browser; any other scheme is dropped, because the browser would ask before launching another app and a direct open would not.
- `openresearch:` links become in-page navigation, so the page's guards against losing unsaved work still apply. A published project's link carries only its slug, and the app downloads the file from its own API, so a link can never make it fetch from anywhere else.

## Updates

Every surface discovers releases through a small manifest at a release download link, a CDN redirect with no API rate limit, because the addresses agents run from exhaust the REST API's. **Manifests serve every client in the wild, so their names and fields only grow.** The macOS and Linux apps read their own, since their packages attach after the release and the CLI's version can run ahead of them. A manifest goes up only after the assets it names; a missing app manifest means nothing to update to yet.

How an update applies depends on the install (`src/updates.rs`, `src/updates/`, `src/commands/update.rs`):

| Installed by | Applies its own updates by |
| --- | --- |
| the shell or PowerShell installer | running the installer again; a receipt proves the binary is the installer's |
| the macOS app | checking the DMG's hash, pinned signature and notarization, renaming the bundle into place |
| the AppImage | checking the new file's hash, renaming it over the old one |
| the Windows app or a zip's `orx.exe` | staging the new `orx.exe`, then two renames |
| cargo, Homebrew, Nix, anything else | never; writing over a package manager's files would break it |

**Replacement is always a rename.** A running process keeps its old file and code signature until it restarts; nothing is ever half-written underneath it.

**An update never swaps the binary under a running `orx up`.** Each server holds a shared lock on its install for its whole life, and an updater needs it exclusively, because agents would otherwise get a different `orx` than the server running them. Under a running server a desktop install stages the verified release beside itself and a CLI install only records it. A terminal or headless server relaunches into it once idle; the desktop app's server only when its page asks ([dashboard](dashboard.md)).

**Channels.** The desktop apps follow stable releases (`v1.2.3`) or canary builds (`canary-YYYYMMDD.N`). A build moves to a canary, or from a canary to stable, only when the target's history contains its own commit, so a switch never moves an install backward. Switching is refused while an installed update waits for a restart.

## The build channel

A build is **production** only when it is an official release or canary built in the project's own GitHub Actions; every other build is **development**. `build.rs` enforces this and requires production builds to embed `orx-host`. Only production builds send telemetry and install released binaries on remote machines. A locally built desktop app never replaces itself with a release.

## How a version ships

**The release act is merging a reviewed pull request that bumps the version in `Cargo.toml`.** Nothing pushes tags. Workflows are in `.github/workflows/`; setup, recovery and the signing identity's contract with installed apps are in `macos/DISTRIBUTION.md`.

1. A push to main that changes the version makes `release-on-bump.yml` dispatch `release.yml`.
2. `release.yml` builds and signs every CLI target, with `ci.yml` beside the builds on the packaged commit.
3. Once CI, `verify-build-channel.yml` and `telemetry-contract.yml` pass, it publishes the release, creating the tag.
4. `release-macos-app.yml`, `release-windows-app.yml` and `release-linux-app.yml` follow, each gated on a repository variable, and attach the desktop packages.

Every push to main also runs `release-canary.yml`: desktop builds beside CI on that commit, published once both pass, possibly skipping intermediate commits. No pull request job builds an AppImage or the Windows installer, so packaging first runs here.

- **Nothing publishes untested code.** `dist-workspace.toml` keeps the checks among dist's global-artifacts jobs, so a regenerated `release.yml` still gates on them. `dist generate` otherwise skips that hand-edited file.
- **Signing runs in the `release-signing` environment**, for canaries too. Windows signs the CLI archive before dist checksums it, so self-updates keep a signed `orx.exe`.

## The gate on main

Main requires `fmt, clippy, test` and `version sanity`, administrators included. Branch protection matches those job names in `.github/workflows/ci.yml`, so renaming either blocks every merge. There is no merge queue and no requirement to be up to date with main.

- Pull request CI tests GitHub's simulated merge. `scripts/ci-changes.mjs` selects jobs by changed files, and `fmt, clippy, test` fails on any failed job and on any job skipped that those files should have run.

## Scripts and workflows

`build-macos-app.sh` and `package-macos-app.sh` sign and notarize the app and DMG (`macos/DISTRIBUTION.md`), `build-linux-appimage.sh` builds the AppImage, and `build-host.mjs` builds the `orx-host` binaries that official builds embed through `ORX_EMBED_HOST_DIR`. `generate-*` draw brand assets and the demo's evidence; `ci-*`, `canary-*`, `change-counts.mjs`, `release-source.sh`, `find-release-run.sh` and `release_notes.py` serve the workflows.

- **Workflows pass `actionlint` and `zizmor --offline`** (with `.github/zizmor.yml`), and every action is pinned to a commit.
- **CI runs current runner images and toolchains.** Pin the floor that matters, such as `MACOSX_DEPLOYMENT_TARGET` in `.cargo/config.toml`, never an old image.
