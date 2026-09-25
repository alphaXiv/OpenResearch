# Linux

## The desktop app

From [Releases](https://github.com/alphaXiv/OpenResearch/releases), download
`OpenResearch-x86_64.AppImage` (or `OpenResearch-aarch64.AppImage` on ARM), make
it executable, and run it:

```sh
chmod +x OpenResearch-x86_64.AppImage
./OpenResearch-x86_64.AppImage
```

The dashboard opens in its own window. Closing the window quits OpenResearch, and
starting it again while it runs brings the window back. Keep the AppImage
somewhere you can write to, such as `~/Applications`: it updates itself by
replacing that file, and Restart in the dashboard relaunches the new version.

The AppImage bundles WebKitGTK, so it needs nothing installed beyond glibc 2.35
or newer (Ubuntu 22.04, Debian 12, Fedora 36, and later) and `fusermount` to
mount itself. Where FUSE isn't available, run it with
`--appimage-extract-and-run`, or set `APPIMAGE_EXTRACT_AND_RUN=1`.

The app starts `orx app` from inside the image. Agents find `orx` because its
folder leads their `PATH`, and because a desktop launcher doesn't read your
shell's startup files, the app asks your login shell for `PATH` and the other
variables orx cares about when it starts. The browser, file manager, editors,
terminals, and agents it opens get your session's GTK settings back, not the AppImage's. It uses port
4792, or a free port if something else holds it.

## The CLI

The release's `openresearch-cli-<arch>-unknown-linux-musl` archives and the shell
installer are static binaries with no GUI; `orx up` opens the dashboard in your
browser.

## Known gaps

| | |
|---|---|
| Audio and video previews | WebKitGTK plays media through GStreamer, which the AppImage doesn't bundle. |
| Error dialogs | If the app can't start, it says why through `zenity` or `kdialog`, and on stderr; without either, run the AppImage from a terminal to see it. |
| Wayland | The bundled GTK runs under XWayland, as linuxdeploy's GTK plugin sets it up to. |

## Releasing the app

`release-linux-app.yml` builds both AppImages from the release's commit, attaches
them, and writes `linux-app.json`, once the repository variable
`LINUX_APP_ENABLED` is `true`. Like the macOS app it follows a Release run
dispatched by a token (see `macos/DISTRIBUTION.md`); to attach the AppImages to an
existing release, dispatch the workflow with its tag. CI on every pull request
also builds, smoke-tests, and uploads both AppImages
(`openresearch-linux-<arch>-appimage`). The smoke test runs after WebKitGTK is
removed from the runner, so it fails if the image falls back to a host copy.

To build one locally on Ubuntu 22.04 or newer:

```sh
sudo apt-get install libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev file patchelf
cargo build --release --features desktop
scripts/build-linux-appimage.sh target/release/orx dist
```
