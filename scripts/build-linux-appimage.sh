#!/usr/bin/env bash
# Package an `orx` built with `--features desktop` into OpenResearch-<arch>.AppImage,
# bundling WebKitGTK so the app runs without it installed. Run on the oldest
# glibc to support (CI uses Ubuntu 22.04) with libwebkit2gtk-4.1-dev installed.
#
#   scripts/build-linux-appimage.sh <orx binary> <output dir>
set -euo pipefail

ORX="$(realpath "$1")"
OUT="$(mkdir -p "$2" && realpath "$2")"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ARCH="$(uname -m)"
TRIPLET="$ARCH-linux-gnu"
WEBKIT_DIR="/usr/lib/$TRIPLET/webkit2gtk-4.1"

LINUXDEPLOY_URL="https://github.com/linuxdeploy/linuxdeploy/releases/download/1-alpha-20251107-1/linuxdeploy-$ARCH.AppImage"
PLUGIN_GTK_URL="https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh"
APPIMAGETOOL_URL="https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$ARCH.AppImage"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
TOOLS="$WORK/tools"
APPDIR="$WORK/AppDir"
mkdir -p "$TOOLS"

echo "==> Fetching linuxdeploy, its GTK plugin, and appimagetool"
curl -fsSL -o "$TOOLS/linuxdeploy" "$LINUXDEPLOY_URL"
curl -fsSL -o "$TOOLS/linuxdeploy-plugin-gtk.sh" "$PLUGIN_GTK_URL"
curl -fsSL -o "$TOOLS/appimagetool" "$APPIMAGETOOL_URL"
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
  --executable "$APPDIR$WEBKIT_DIR/WebKitNetworkProcess" \
  --executable "$APPDIR$WEBKIT_DIR/WebKitWebProcess" \
  --library "$APPDIR$WEBKIT_DIR/injected-bundle/libwebkit2gtkinjectedbundle.so" \
  --desktop-file "$ROOT/linux/OpenResearch.desktop" \
  --icon-file "$ROOT/linux/OpenResearch.png" \
  --custom-apprun "$ROOT/linux/AppRun" \
  --plugin gtk

# The same-length rewrite Tauri uses: WebKit's absolute /usr paths become ././,
# which AppRun resolves inside the image by running from $APPDIR/usr.
find "$APPDIR/usr/lib" -name 'libwebkit*' -exec sed -i -e 's|/usr|././|g' '{}' +

echo "==> Building the AppImage"
ARCH="$ARCH" appimagetool "$APPDIR" "$OUT/OpenResearch-$ARCH.AppImage"
echo "==> Done: $OUT/OpenResearch-$ARCH.AppImage"
