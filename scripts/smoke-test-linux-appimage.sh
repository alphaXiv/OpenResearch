#!/usr/bin/env bash
# Test automatic system-WebKit selection, the bundled-WebKit fallback, or the
# missing-media dependency dialog. The probe uses the selected desktop runtime.
#
# scripts/smoke-test-linux-appimage.sh <AppImage> <media probe> [system|bundled|missing]
set -euo pipefail

APPIMAGE="$(realpath "$1")"
MEDIA_PROBE="$(realpath "$2")"
MODE="${3:-system}"
case "$MODE" in system|bundled|missing) ;; *) echo "Unknown runtime test mode: $MODE" >&2; exit 1 ;; esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SANDBOX="$(mktemp -d)"
LOG="$SANDBOX/app.log"
export ORX_DATA_DIR="$SANDBOX/data" XDG_CONFIG_HOME="$SANDBOX/config" XDG_DATA_HOME="$SANDBOX/share"
export HOME="$SANDBOX/home" XDG_CACHE_HOME="$SANDBOX/cache" XDG_RUNTIME_DIR="$SANDBOX/runtime"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
# GTK must use Xvfb, not the caller's Wayland display or app-instance socket.
unset WAYLAND_DISPLAY WAYLAND_SOCKET
export ORX_NO_UPDATE_CHECK=1 ORX_TELEMETRY_ENV=off
# No FUSE on CI runners; the AppImage runs from a temporary extraction instead.
export APPIMAGE_EXTRACT_AND_RUN=1
# Cleanup kills the runtime before it removes its extraction, so extract here.
export TMPDIR="$SANDBOX"

# The hook changes the theme and data directories and forces X11. AppRun must
# save their original values before sourcing it, and leave backend choice to GTK.
unset GDK_BACKEND LD_LIBRARY_PATH
export GTK_THEME=Adwaita:dark APPIMAGE_GTK_THEME=Adwaita:light
export XDG_DATA_DIRS=/usr/local/share:/usr/share

# Its own process group, so cleanup reaches orx and WebKit, not just xvfb-run.
setsid xvfb-run -a "$APPIMAGE" >"$LOG" 2>&1 &
APP=$!
cleanup() {
  kill -- -"$APP" 2>/dev/null || true
  for _ in $(seq 1 30); do
    kill -0 -- -"$APP" 2>/dev/null || break
    sleep 0.1
  done
  kill -KILL -- -"$APP" 2>/dev/null || true
  wait "$APP" 2>/dev/null || true
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

if [ "$MODE" = missing ]; then
  dialog=
  for _ in $(seq 1 45); do
    for pid in $(pgrep -f 'usr/bin/orx-runtime error' || true); do
      grep -zqxF "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" "/proc/$pid/environ" 2>/dev/null || continue
      X_ENV="$(tr '\0' '\n' <"/proc/$pid/environ" | grep -E '^(DISPLAY|XAUTHORITY)=')"
      # shellcheck disable=SC2086
      dialog="$(env $X_ENV xdotool search --onlyvisible --name '^OpenResearch needs multimedia libraries$' || true)"
      [ -n "$dialog" ] && break
    done
    [ -n "$dialog" ] && break
    kill -0 "$APP" 2>/dev/null || fail "No dependency dialog appeared."
    sleep 1
  done
  [ -n "$dialog" ] || fail "No dependency dialog appeared."
  grep -q 'sudo apt install libwebkit2gtk-4.1-0' "$LOG" || fail "No install instructions were printed."
  # shellcheck disable=SC2086
  env $X_ENV xdotool windowfocus "$dialog" key Escape || true # Xvfb can exit before key release.
  for _ in $(seq 1 10); do
    kill -0 "$APP" 2>/dev/null || break
    sleep 1
  done
  kill -0 "$APP" 2>/dev/null && fail "Escape did not close the dependency dialog."
  status=0
  wait "$APP" || status=$?
  [ "$status" -eq 1 ] || fail "The missing-runtime launch returned $status, expected 1."
  echo "Missing media dependencies produce an actionable dialog and a failed launch."
  exit 0
fi

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
ORX_PID=
for pid in $(pgrep -f 'usr/bin/orx app' || true); do
  if grep -zqxF "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" "/proc/$pid/environ" 2>/dev/null; then ORX_PID="$pid"; break; fi
done
[ -n "$ORX_PID" ] || fail "orx app is not running."
APP_ENV="$(tr '\0' '\n' <"/proc/$ORX_PID/environ")"
for entry in 'ORX_HOST_GDK_BACKEND=-' 'ORX_HOST_GTK_THEME==Adwaita:dark' \
  'ORX_HOST_XDG_DATA_DIRS==/usr/local/share:/usr/share' 'ORX_HOST_LD_LIBRARY_PATH=-' "ORX_APPIMAGE_RUNTIME=$MODE"; do
  grep -qxF "$entry" <<<"$APP_ENV" || fail "AppRun did not preserve the host environment: $entry"
done
if [ "$MODE" = system ]; then
  grep -qxF 'GTK_THEME=Adwaita:dark' <<<"$APP_ENV" || fail "The system runtime changed the host theme."
else
  grep -qxF 'GTK_THEME=Adwaita:light' <<<"$APP_ENV" || fail "The bundled GTK hook did not run."
fi
if grep -q '^GDK_BACKEND=' <<<"$APP_ENV"; then
  fail "AppRun forced a display backend instead of letting GTK select it."
fi
X_ENV="$(tr '\0' '\n' <"/proc/$ORX_PID/environ" | grep -E '^(DISPLAY|XAUTHORITY)=' || true)"
visible=
for _ in $(seq 1 30); do
  grep -q 'showing the window anyway' "$LOG" && fail "The dashboard never finished loading."
  # shellcheck disable=SC2086
  if env $X_ENV xdotool search --onlyvisible --class '^OpenResearch$' >/dev/null 2>&1; then
    visible=1
    break
  fi
  sleep 1
done
[ -n "$visible" ] || fail "No window of class OpenResearch appeared."
grep -q 'showing the window anyway' "$LOG" && fail "The dashboard never finished loading."
grep -qxF "TryExec=$APPIMAGE" "$XDG_DATA_HOME/applications/openresearch.desktop" \
  || fail "The app did not install its desktop entry."

mapfile -d '' -t media_env <"/proc/$ORX_PID/environ"
if [ "$MODE" = bundled ]; then
  image_exe="$(readlink "/proc/$ORX_PID/exe")"
  media_env+=("LD_LIBRARY_PATH=$(dirname "$(dirname "$image_exe")")/lib")
fi
if ! (
  ulimit -c 0
  cd "/proc/$ORX_PID/cwd"
  timeout 35 env -i "${media_env[@]}" \
    "$MEDIA_PROBE" "$ROOT/scripts/fixtures/appimage-media.html"
) >"$SANDBOX/media.log" 2>&1; then
  cat "$SANDBOX/media.log"
  fail "The selected WebKit could not render sandboxed HTML audio/video."
fi
cat "$SANDBOX/media.log"

helpers=0
for pid in $(pgrep -f 'WebKit(Web|Network)Process'); do
  grep -zqxF "XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR" "/proc/$pid/environ" 2>/dev/null || continue
  # WebKit may replace a web process between pgrep and here.
  exe="$(readlink "/proc/$pid/exe")" || continue
  maps="$(cat "/proc/$pid/maps" 2>/dev/null)" || continue
  helpers=$((helpers + 1))
  if [ "$MODE" = system ]; then
    case "$exe" in *appimage_extracted_*) fail "The system runtime started a bundled helper." ;; esac
    if grep -Eq 'appimage_extracted_.*/lib(webkit|glib|gst)' <<<"$maps"; then
      fail "A system WebKit helper loaded the bundled runtime."
    fi
  else
    case "$exe" in *appimage_extracted_*) ;; *) fail "A bundled helper runs $exe." ;; esac
    grep -q 'appimage_extracted_.*/libwebkit2gtk' <<<"$maps" || fail "Bundled WebKit was not loaded."
    if grep -q ' /usr/lib/[^ ]*/gio/modules/' <<<"$maps"; then
      fail "The bundled helper loaded the host's GIO modules."
    fi
  fi
done
[ "$helpers" -gt 0 ] || fail "No WebKit helper could be inspected."
echo "The $MODE WebKit loaded the dashboard and media:"
pgrep -af 'WebKit(Web|Network)Process'
