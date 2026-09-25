#!/usr/bin/env bash
# Package an `orx` built with `--features desktop` into OpenResearch-<arch>.AppImage,
# bundling WebKitGTK so the app runs without it installed. Run on the oldest
# glibc to support (CI uses Ubuntu 22.04) with libwebkit2gtk-4.1-dev and
# patchelf installed.
#
#   scripts/build-linux-appimage.sh <orx binary> <output dir>
set -euo pipefail

ORX="$(realpath "$1")"
OUT="$(mkdir -p "$2" && realpath "$2")"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="$(uname -m)"
TRIPLET="$ARCH-linux-gnu"
WEBKIT_DIR="/usr/lib/$TRIPLET/webkit2gtk-4.1"

# Pinned, and checked against the digests GitHub publishes for each release asset.
LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-$ARCH.AppImage"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$ARCH.AppImage"
RUNTIME_URL="https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$ARCH"
# A commit-pinned raw URL is already content-addressed.
PLUGIN_GTK_URL="https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh"
case "$ARCH" in
  x86_64)
    LINUXDEPLOY_SHA256=c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d
    APPIMAGETOOL_SHA256=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    RUNTIME_SHA256=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d
    ;;
  aarch64)
    LINUXDEPLOY_SHA256=620095110d693282b8ebeb244a95b5e911cf8f65f76c88b4b47d16ae6346fcff
    APPIMAGETOOL_SHA256=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    RUNTIME_SHA256=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444
    ;;
  *)
    echo "No AppImage tooling pinned for $ARCH" >&2
    exit 1
    ;;
esac

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TOOLS="$WORK/tools"
APPDIR="$WORK/AppDir"
mkdir -p "$TOOLS"

fetch() {
  curl -fsSL -o "$1" "$2"
  if [ -n "${3:-}" ]; then
    echo "$3  $1" | sha256sum -c --quiet -
  fi
}

echo "==> Fetching linuxdeploy, its GTK plugin, appimagetool, and the AppImage runtime"
fetch "$TOOLS/linuxdeploy" "$LINUXDEPLOY_URL" "$LINUXDEPLOY_SHA256"
fetch "$TOOLS/linuxdeploy-plugin-gtk.sh" "$PLUGIN_GTK_URL"
fetch "$TOOLS/appimagetool" "$APPIMAGETOOL_URL" "$APPIMAGETOOL_SHA256"
fetch "$WORK/runtime" "$RUNTIME_URL" "$RUNTIME_SHA256"
chmod +x "$TOOLS"/*
# linuxdeploy finds its plugins on PATH; the runners have no FUSE to mount the tools.
export PATH="$TOOLS:$PATH"
export APPIMAGE_EXTRACT_AND_RUN=1

echo "==> Staging the AppDir"
install -Dm755 "$ORX" "$APPDIR/usr/bin/orx"
# WebKit starts these helpers from a path compiled into libwebkit2gtk, which the
# sed below makes relative to AppRun's working directory, $APPDIR/usr.
for helper in WebKitNetworkProcess WebKitWebProcess injected-bundle/libwebkit2gtkinjectedbundle.so; do
  install -Dm755 "$WEBKIT_DIR/$helper" "$APPDIR$WEBKIT_DIR/$helper"
done

echo "==> Deploying libraries with linuxdeploy"
DEPLOY_GTK_VERSION=3 linuxdeploy \
  --appdir "$APPDIR" \
  --executable "$APPDIR/usr/bin/orx" \
  --desktop-file "$ROOT/linux/OpenResearch.desktop" \
  --icon-file "$ROOT/linux/OpenResearch.png" \
  --custom-apprun "$ROOT/linux/AppRun" \
  --plugin gtk

# linuxdeploy leaves the in-place helpers looking beside themselves, so they
# would load the host's libwebkit (or none); point them at the bundled usr/lib.
patchelf --set-rpath '$ORIGIN/../..' \
  "$APPDIR$WEBKIT_DIR/WebKitNetworkProcess" "$APPDIR$WEBKIT_DIR/WebKitWebProcess"
patchelf --set-rpath '$ORIGIN/../../..' \
  "$APPDIR$WEBKIT_DIR/injected-bundle/libwebkit2gtkinjectedbundle.so"

# The same-length rewrite Tauri uses: WebKit's absolute /usr paths become ././,
# which AppRun resolves inside the image by running from $APPDIR/usr.
find "$APPDIR/usr/lib" -name 'libwebkit*' -exec sed -i -e 's|/usr|././|g' '{}' +

echo "==> Building the AppImage"
ARCH="$ARCH" appimagetool --runtime-file "$WORK/runtime" "$APPDIR" "$OUT/OpenResearch-$ARCH.AppImage"
echo "==> Done: $OUT/OpenResearch-$ARCH.AppImage"
