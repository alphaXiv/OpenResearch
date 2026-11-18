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
