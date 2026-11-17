# Linux desktop runtime

OpenResearch prefers the system's WebKitGTK 4.1 libraries so HTML previews use media plugins built for the same runtime. AppRun finds WebKit through `ldconfig -p` and checks that the desktop binary can load against that directory. If it cannot, the existing bundled WebKit starts with its GTK hooks. Host programs launched by OpenResearch receive the session's original library path.

Before opening the dashboard, the app runs its hidden `check-media` command as a child with the selected runtime. Plugin scanning stays inside that child. After the three-second timeout it may keep scanning for 30 seconds, so a slow first scan still saves GStreamer's registry for the next launch, and is then killed. The Rust GStreamer API checks the base elements needed by audio, Web Audio and video. Missing optional codecs remain ordinary playback errors. A failed, crashed or timed-out check disables media, Web Audio and MediaSource in the webview before loading any page. The dashboard and ordinary HTML remain available. Audio/video previews offer a download and installation instructions; HTML previews that use video, audio or Web Audio show those instructions above the document.

To enable audio and video, install the system runtime and media packages, then reopen OpenResearch:

Ubuntu / Debian:

```sh
sudo apt install libwebkit2gtk-4.1-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav
```

Arch Linux:

```sh
sudo pacman -S --needed webkit2gtk-4.1 gst-plugins-base-libs gst-plugins-good gst-libav
```

No global library-path override is needed. Installing newer plugins alone does not make them compatible with an older bundled GStreamer. Other video formats may need additional codecs.

Media plugins and codec dependencies stay in shared system packages; they are not added to each AppImage. Stable and Canary builds test the system runtime, system WebKit without `gst-plugins-good`, the bundled fallback, and the bundled fallback without media plugins. They also check system-library selection without `/sbin` on PATH, and use a test-only Rust GStreamer plugin to verify crash and timeout isolation and child cleanup. That plugin is not packaged. The smoke test opens a fixture in the AppImage's own HTML preview, checking Web Audio initialization and a decoded H.264 frame. For binaries predating `check-media`, older release rebuilds check dashboard startup and explicitly skip the new media and degradation checks.
