#!/usr/bin/env bash
# Launch a built AppImage under a virtual display and prove its bundled WebKit
# loads the dashboard: the server answers, WebKit's network process has connected
# to it, the window appears once the page has loaded, and every WebKit helper runs
# from inside the image on the image's libwebkit2gtk. Run it where WebKitGTK is not
# installed, or a fallback to the host's copy would pass. Needs xvfb-run, curl, ss,
# setsid, and xdotool.
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
# Cleanup kills the runtime before it removes its extraction, so extract here.
export TMPDIR="$SANDBOX"

# Its own process group, so cleanup reaches orx and WebKit, not just xvfb-run.
setsid xvfb-run -a "$APPIMAGE" >"$LOG" 2>&1 &
APP=$!
cleanup() {
  kill -- -"$APP" 2>/dev/null || true
  # orx may still be shutting down and writing into it.
  rm -rf "$SANDBOX" || true
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

connected=
for _ in $(seq 1 60); do
  # ss truncates process names to 15 characters: WebKitNetworkPr.
  if ss -tnp 2>/dev/null | grep ':4792' | grep -q 'WebKitNetwork'; then
    connected=1
    break
  fi
  sleep 1
done
[ -n "$connected" ] || fail "WebKit never connected to the dashboard."
pgrep -f WebKitWebProcess >/dev/null || fail "No WebKit web process is running."

# The app shows its window when the page finishes loading, or says it gave up.
ORX_PID="$(pgrep -o -f 'usr/bin/orx app' || true)"
[ -n "$ORX_PID" ] || fail "orx app is not running."
X_ENV="$(tr '\0' '\n' <"/proc/$ORX_PID/environ" | grep -E '^(DISPLAY|XAUTHORITY)=' || true)"
visible=
for _ in $(seq 1 30); do
  grep -q 'showing the window anyway' "$LOG" && fail "The dashboard never finished loading."
  # shellcheck disable=SC2086
  if env $X_ENV xdotool search --onlyvisible --name '^OpenResearch$' >/dev/null 2>&1; then
    visible=1
    break
  fi
  sleep 1
done
[ -n "$visible" ] || fail "The window never appeared."

for pid in $(pgrep -f 'WebKit(Web|Network)Process'); do
  # WebKit may replace a web process between pgrep and here.
  exe="$(readlink "/proc/$pid/exe")" || continue
  maps="$(cat "/proc/$pid/maps" 2>/dev/null)" || continue
  case "$exe" in
    *appimage_extracted_*) ;;
    *) fail "WebKit helper $pid runs $exe, not the AppImage's." ;;
  esac
  grep -q 'appimage_extracted_.*/libwebkit2gtk' <<<"$maps" \
    || fail "WebKit helper $pid did not load the AppImage's libwebkit2gtk."
done
echo "The AppImage's WebKit loaded the dashboard:"
pgrep -af 'WebKit(Web|Network)Process'
