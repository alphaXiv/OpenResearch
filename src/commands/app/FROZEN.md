# Frozen app window surfaces

The desktop window outlives the server it opened on: an update replaces the server and leaves the
window running the build it started with until the user quits, so a window meets every later
server, and every later page. Everything here is therefore a contract between builds: it may only
grow. Add optional fields, routes, messages and arguments; never rename, retype or repurpose one.
Each side ignores what it does not know. [desktop](../../../arc/desktop.md#keeping-the-window-through-an-update)
says why the pieces exist; this file says what each build must keep doing.

## The update overlay

A page that restarts its server into an update shows an overlay that hides the page entirely, saves
a record of it, and reloads once the next server answers. The next build's `index.html` loads
`ui/public/update-overlay.js` before its first paint, and that script draws the overlay again from
the record, so the reload never shows. The next build's app then takes the overlay down, or says the
update did not install.

| Surface | Contract |
|---|---|
| Record | `sessionStorage["orx:applying-update"]`: JSON `{from, to, title, body, captions, at}`. `from` is the restarting server's `build` and `to` the `installedVersion` it restarts into, both as `GET /api/update` names them; `title` and `body` are the overlay's text and `captions` the names of the Windows caption buttons, `{minimize, maximize, restore, close}` or `null`, all already translated; `at` is when it was saved, in Unix milliseconds. A record more than ten minutes old is ignored. |
| Outcome | The next page compares its server's `build` with the record: `from` means the update did not install, anything else that it did. |
| Look | A full-window cover in the page's background colour, centred over it the dialog card of `ui/src/components/ui/Dialog.tsx`: a title, then a spinner and the body; on Windows, the caption buttons of `ui/src/components/WindowControls.tsx` in the cover's top-right corner, posting `titlebar:minimize`, `titlebar:zoom` and `titlebar:close`. The spinner's phase follows the clock. A build that changes any of this shows a visible jump on the one update into it. |

## The AppImage's server entry

`"$APPIMAGE" server ARGS...` runs the image's own `orx` with `ARGS`, in the environment `linux/AppRun`
prepares for the app. Its caller restores the session's values first
(`shell_env::restore_host_gui_env`), so AppRun records the session's values again rather than the
image's. A window starts its server this way, so the server runs from a mount of its own that
outlives the window's, and a server restarts into an update this way, from the image now at that
path. Every AppRun keeps this entry, because a window started from any earlier image runs it on the
newest one.

## The window's server

A window starts its server as a child and controls it over HTTP on the port the window chose.
After an update the server is a newer build than the window, so every server keeps answering what
every older window sends.

| Surface | Contract |
|---|---|
| Start | `orx [--no-telemetry] up --app-window --port PORT`, from the window's own executable path, or on Linux `"$APPIMAGE" server` and the same arguments. The server binds `127.0.0.1:PORT`, opens no browser and restarts only when its page asks. |
| Lease | The server's stdin is a pipe whose other end the window holds and never writes. End of input or a broken pipe means the window is gone without quitting: the server hands its work off as for a restart and exits, and the next launch adopts it. |
| Stderr | A pipe the window reads to the end, and passes on to its own. The server shows no alert of its own: one that cannot go on writes why, for the user, as its last line and exits with status 1. |
| Restart | A server restarting into an update keeps the lease and the port. On Unix it execs in place, so its pid stays. On Windows it starts its successor, which inherits stdin and stderr, names it in the runtime directory's `relaunch.json` as `{until, pid}` and exits with status 0; the successor publishes `server.json` with the same `pid`, which ends the announcement. A window opens each successor by that `pid` as soon as `relaunch.json` names it, while the server that named it still holds it, or, after the server's exit with status 0, from `server.json`; it reads the successor's exit as the server's, and treats an exit with status 0 and no successor as the server gone. |
| `GET /api/pending-work` | `{agents, runs}`, as `PendingWork` in `ui/src/api.ts`. The window passes it whole to the page's close dialog, and lists what it can read in a native alert when the page cannot ask. Both lists empty means a quit interrupts nothing. |
| `GET /api/app-window` | `{busy}`: whether quitting would interrupt work or close a page someone has open. A window with no page shown quits once this has been false for ten minutes. |
| `POST /api/app-window/quit` | Answers `204`, asks the runs that need OpenResearch open to stop for at most ten seconds, stops everything else and exits. A window waits a while for the exit, then leaves anyway. |

The window's requests carry no `Origin`, so the loopback guard admits them as any local client's.
Only a server started with `--app-window` has the `/api/app-window` routes.

An app from before the window outlived its server relaunches into a newer build as a whole: with
`ORX_APP_RELAUNCH_PORT=PORT` on every platform, and `ORX_RELAUNCH_WAIT_PID` on Windows, whose new
window waits for that process to exit. A window keeps that port.

## The page and the window

After an update a window shows pages newer than itself, so the bridge between them only grows too.
A window ignores a message it does not know.

| Message | Contract |
|---|---|
| `orx:server-stopped` | A window event: the window's server exited and nothing took over. Its `detail` is `{reason, log}`: `reason` the server's last line on stderr when it exited with status 1, else `null`; `log` the rest of the end of its stderr, then how it exited. The page says why it stopped, offers to start it again or to quit, and says so with `server:asking`; a window that hears nothing within a second asks in a native alert instead. |
| `server:start` | To the window: start the server again on the same port. The page reloads once a server answers. |
| `app:quit` | To the window: quit, as the app's own Quit command does. |
