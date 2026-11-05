#!/usr/bin/env bash
set -euo pipefail
test "$WORKFLOW_REF" = refs/heads/main
sha=$(git rev-parse HEAD)
git merge-base --is-ancestor "$sha" origin/main
version=$(grep -m1 '^version = ' Cargo.toml | sed -E 's/version = "(.*)"/\1/')
test "$RELEASE_TAG" = "v$version"
echo "sha=$sha" >> "$GITHUB_OUTPUT"
jq -n --arg tag "$RELEASE_TAG" --arg sha "$sha" '{tag: $tag, sha: $sha}' > release-source.json
