#!/usr/bin/env bash
# Launch a built AppImage under a virtual display and prove the bundled WebKit
# loads the dashboard: the server answers, and WebKit's network process, started
# from inside the image, holds connections to it. Needs xvfb-run, curl, and ss.
#
#   scripts/smoke-test-linux-appimage.sh <OpenResearch-<arch>.AppImage>
set -euo pipefail

APPIMAGE="$(realpath "$1")"
SANDBOX="$(mktemp -d)"
LOG="$SANDBOX/app.log"
export ORX_DATA_DIR="$SANDBOX/data" XDG_CONFIG_HOME="$SANDBOX/config"
export ORX_NO_UPDATE_CHECK=1 ORX_TELEMETRY_ENV=off
# No FUSE on CI runners; the AppImage runs from a temporary extraction instead.
export APPIMAGE_EXTRACT_AND_RUN=1

xvfb-run -a "$APPIMAGE" >"$LOG" 2>&1 &
APP=$!
cleanup() {
  kill "$APP" 2>/dev/null || true
  pkill -f "$SANDBOX" 2>/dev/null || true
  rm -rf "$SANDBOX"
}
trap cleanup EXIT

fail() {
  echo "::error::$1"
  echo "--- app log ---"
  cat "$LOG"
  exit 1
}

for _ in $(seq 1 120); do
  curl -fsS -o /dev/null http://127.0.0.1:4792/api/health && break
  kill -0 "$APP" 2>/dev/null || fail "The app exited before its dashboard came up."
  sleep 1
done
curl -fsS -o /dev/null http://127.0.0.1:4792/api/health || fail "The dashboard never answered on 4792."

for _ in $(seq 1 60); do
  if ss -tnp 2>/dev/null | grep ':4792' | grep -q 'WebKitNetwork'; then
    echo "WebKit loaded the dashboard:"
    pgrep -af 'WebKit(Web|Network)Process' || true
    exit 0
  fi
  sleep 1
done
fail "WebKit never connected to the dashboard."
