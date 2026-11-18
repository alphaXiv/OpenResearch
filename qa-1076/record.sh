#!/usr/bin/env bash
set -euo pipefail
export EVIDENCE="$RUNNER_TEMP/evidence"
cat > "$RUNNER_TEMP/run-proof.sh" <<'TERM'
#!/usr/bin/env bash
set -euo pipefail
exec > >(tee "$EVIDENCE/trace.txt") 2>&1
echo 'OpenResearch setup diagnostics | Linux x86_64'
echo 'Real setup WebSocket, real PTY, isolated telemetry receiver.'
echo 'Synthetic OpenCode provider; no real accounts or user data.'
echo
echo 'BEFORE  2567ddf9  (main)'
python3 -u qa-1076/prove.py "$RUNNER_TEMP/baseline/orx" before
sleep 1
echo
echo 'AFTER   12cc022e  (PR #1076)'
python3 -u qa-1076/prove.py "$GITHUB_WORKSPACE/target/debug/orx" after
echo
echo 'Both app-level assertions passed. No production endpoint used.'
echo 0 > "$EVIDENCE/exit"
sleep 2
TERM
ffmpeg -hide_banner -loglevel error -y -f x11grab -framerate 8 -video_size 1280x720 -i "$DISPLAY" -c:v libx264 -preset ultrafast -crf 21 -pix_fmt yuv420p "$EVIDENCE/linux-setup-before-after.mp4" &
rec=$!
trap 'kill -INT "$rec" 2>/dev/null || true; wait "$rec" || true' EXIT
xterm -geometry 114x30+0+0 -fa 'DejaVu Sans Mono' -fs 13 -fg '#e8edf3' -bg '#141922' -e /bin/bash "$RUNNER_TEMP/run-proof.sh"
cat "$EVIDENCE/trace.txt"
test -f "$EVIDENCE/exit"
