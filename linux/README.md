# Linux desktop runtime

OpenResearch prefers the system's WebKitGTK 4.1 and GStreamer libraries so HTML previews use media plugins built for the same runtime. Installing new system plugins does not make them compatible with the older GStreamer inside an AppImage.

Install the runtime and common video codecs before opening the AppImage:

Ubuntu / Debian:

```sh
sudo apt install libwebkit2gtk-4.1-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-libav
```

Arch Linux:

```sh
sudo pacman -S --needed webkit2gtk-4.1 gst-plugins-base-libs gst-plugins-good gst-libav
```

AppRun discovers the library directory, checks the required media elements and verifies the desktop binary's imports before selecting the system stack. It leaves the session's GTK settings in place. No global `LD_LIBRARY_PATH` setting or manual launch command is needed, and host programs launched by OpenResearch receive the session's original library path.

If the system stack is unavailable, the existing bundled WebKit can still run with compatible system media plugins, as on Ubuntu 22.04. If neither stack has the required media elements, a dialog gives installation commands before the dashboard starts. A video format can still require additional codecs; the libav package above supplies the H.264/AAC support exercised by the package test.

The fallback keeps its previous bundle size: media plugins and their codec dependencies are not added to the AppImage. Both stable and Canary builds test system selection, the bundled fallback after removing system WebKit, and the dependency dialog after removing media plugins.
