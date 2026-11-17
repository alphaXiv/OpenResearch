#!/usr/bin/env bash
# Exercise HTML media in the AppImage's own desktop window, using private storage.
# scripts/smoke-test-linux-appimage.sh <AppImage> <system|bundled>[-disabled|-abort|-hang] [fault-plugin-dir]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 "$ROOT/scripts/smoke-test-linux-appimage.py" "$@"
