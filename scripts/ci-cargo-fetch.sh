#!/usr/bin/env bash
set -euo pipefail

host=$(rustc -vV | sed -n 's/^host: //p')
if [ "$#" -eq 0 ]; then
  set -- Cargo.toml
fi
for manifest in "$@"; do
  cargo fetch --manifest-path "$manifest" --locked --offline --target "$host" 2>/dev/null ||
    cargo fetch --manifest-path "$manifest" --locked --target "$host"
done
echo 'CARGO_NET_OFFLINE=true' >> "$GITHUB_ENV"
