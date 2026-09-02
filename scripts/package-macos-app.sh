#!/bin/bash
# Sign, notarize, and package dist/OpenResearch.app into a distributable DMG.
#
# Run scripts/build-macos-app.sh first. This script is env-driven so it works
# both locally (unsigned, for a quick DMG) and in CI (fully signed + notarized):
#
#   MACOS_SIGN_IDENTITY   "Developer ID Application: NAME (TEAMID)".
#                         Unset → skip signing (UNSIGNED dmg; Gatekeeper warns).
#   MACOS_NOTARY_PROFILE  notarytool keychain profile (see `notarytool
#                         store-credentials`). Unset → skip notarization.
#
# See macos/DISTRIBUTION.md for the full setup and the CI wiring.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP="$ROOT/dist/OpenResearch.app"
DMG="$ROOT/dist/OpenResearch.dmg"
EXE="$APP/Contents/MacOS/OpenResearch"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "package-macos-app.sh: macOS only" >&2
  exit 1
fi
if [[ ! -d "$APP" ]]; then
  echo "package-macos-app.sh: $APP not found — run scripts/build-macos-app.sh first" >&2
  exit 1
fi

if [[ -n "${MACOS_SIGN_IDENTITY:-}" ]]; then
  echo "==> Codesigning with hardened runtime"
  # Sign inside-out: the nested executable first, then the bundle.
  ENTITLEMENTS="$ROOT/macos/entitlements.plist"
  codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$MACOS_SIGN_IDENTITY" "$EXE"
  codesign --force --options runtime --timestamp --entitlements "$ENTITLEMENTS" --sign "$MACOS_SIGN_IDENTITY" "$APP"
  codesign --verify --strict --verbose=2 "$APP"
else
  echo "==> MACOS_SIGN_IDENTITY unset — building an UNSIGNED bundle (local test only)."
fi

# Notarize + staple the .app itself first, so it still launches when a user drags
# it out of the DMG and first opens it offline (a DMG-only staple wouldn't cover
# the extracted app).
if [[ -n "${MACOS_NOTARY_PROFILE:-}" ]]; then
  echo "==> Notarizing the app (submitting to Apple; can take a few minutes)"
  APP_ZIP="$ROOT/dist/OpenResearch-app.zip"
  ditto -c -k --keepParent "$APP" "$APP_ZIP"
  xcrun notarytool submit "$APP_ZIP" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
  xcrun stapler staple "$APP"
  rm -f "$APP_ZIP"
fi

echo "==> Creating styled DMG (drag-to-Applications installer window)"
rm -f "$DMG"
VOLNAME="OpenResearch"
# WIN_W/WIN_H must match LOGICAL_W/LOGICAL_H in generate-dmg-background.mjs, and
# APP_X/APPS_X/ICON_Y/ICON constrain the ARROW it draws — change these together.
WIN_W=640 WIN_H=320 ICON=128
APP_X=160 APPS_X=480 ICON_Y=150     # Finder icon-center positions in the window

STAGE="$(mktemp -d)"
BUILD="$(mktemp -d)"
# Detach first so a failure between attach and convert doesn't leak a mount whose
# backing image we're about to delete.
trap '{ [[ -n "${DEV:-}" ]] && hdiutil detach "$DEV" -force >/dev/null 2>&1; } || true; rm -rf "$STAGE" "$BUILD"' EXIT
cp -R "$APP" "$STAGE/"
# Losing this to a dereferencing copy is silent: the app still builds, signs and
# runs, and agents just quietly fall back to the user's own `orx` install.
[[ -L "$STAGE/$(basename "$APP")/Contents/MacOS/orx" ]] || {
  echo "package-macos-app.sh: the orx symlink did not survive staging" >&2
  exit 1
}
ln -s /Applications "$STAGE/Applications"
mkdir "$STAGE/.background"
# HiDPI background: 1x + 2x combined into one multi-representation TIFF.
node "$ROOT/scripts/generate-dmg-background.mjs" "$BUILD/bg.png" 1 >/dev/null
node "$ROOT/scripts/generate-dmg-background.mjs" "$BUILD/bg@2x.png" 2 >/dev/null
tiffutil -cathidpicheck "$BUILD/bg.png" "$BUILD/bg@2x.png" -out "$STAGE/.background/background.tiff" >/dev/null

# Lay the window out on a writable image, then convert to the compressed final.
RW="$BUILD/rw.dmg"
hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ -format UDRW -ov "$RW" >/dev/null
ATTACH="$(hdiutil attach "$RW" -readwrite -noverify -noautoopen)"
DEV="$(echo "$ATTACH" | grep -Eo '^/dev/disk[0-9]+' | head -1)"
MNT="$(echo "$ATTACH" | grep -Eo '/Volumes/.*$' | head -1)"
BG_POSIX="$MNT/.background/background.tiff"
# Styling needs a Finder session; if it fails (e.g. headless), still ship a
# functional DMG (the app + Applications alias are already on the image).
STYLED=1
osascript >/dev/null <<APPLESCRIPT || STYLED=0
tell application "Finder"
  tell disk "$VOLNAME"
    open
    delay 1
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set the bounds of container window to {200, 140, 200 + $WIN_W, 140 + $WIN_H}
    set opts to the icon view options of container window
    set arrangement of opts to not arranged
    set icon size of opts to $ICON
    set text size of opts to 13
    set background picture of opts to POSIX file "$BG_POSIX"
    set position of item "$(basename "$APP")" of container window to {$APP_X, $ICON_Y}
    set position of item "Applications" of container window to {$APPS_X, $ICON_Y}
    close
    open
    update without registering applications
    delay 2
  end tell
end tell
APPLESCRIPT
[[ "$STYLED" == 1 ]] || echo "==> WARNING: Finder styling step failed — shipping a functional but unstyled DMG."
sync
hdiutil detach "$DEV" >/dev/null || { sleep 2; hdiutil detach "$DEV" -force >/dev/null; }
hdiutil convert "$RW" -format UDZO -imagekey zlib-level=9 -o "$DMG" >/dev/null
[[ -n "${MACOS_SIGN_IDENTITY:-}" ]] && codesign --force --timestamp --sign "$MACOS_SIGN_IDENTITY" "$DMG"

if [[ -n "${MACOS_NOTARY_PROFILE:-}" ]]; then
  echo "==> Notarizing and stapling the DMG"
  xcrun notarytool submit "$DMG" --keychain-profile "$MACOS_NOTARY_PROFILE" --wait
  xcrun stapler staple "$DMG"
  xcrun stapler validate "$DMG"
else
  echo "==> MACOS_NOTARY_PROFILE unset — skipping notarization (downloads would warn)."
fi

echo "==> Done: $DMG"
