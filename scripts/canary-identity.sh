#!/usr/bin/env bash
set -euo pipefail
gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases?per_page=100" | jq '[.[][] | .tag_name]' > releases.json
if jq -e 'index("canary") != null' releases.json > /dev/null; then
  release_id=$(gh api "repos/$GITHUB_REPOSITORY/releases/tags/canary" --jq .id)
  gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases/$release_id/assets?per_page=100" | jq '[.[][].name]' > assets.json
else
  printf '[]\n' > assets.json
fi
jq -s '.[0] + .[1]' releases.json assets.json > identities.json
tag=$(node "$(dirname "$0")/canary-tag.mjs" "$(date -u +%Y%m%d)" < identities.json)
version=$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
changes=$(node "$(dirname "$0")/change-counts.mjs" "$GITHUB_SHA")
printf 'tag=%s\nversion=%s\nchanges=%s\n' "$tag" "$version" "$changes" >> "$GITHUB_OUTPUT"
