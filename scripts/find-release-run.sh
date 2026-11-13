#!/usr/bin/env bash
set -euo pipefail

tag="$1"
sha="$2"
dir="$(mktemp -d)"
trap 'rm -rf "$dir"' EXIT

runs=$(gh run list --repo "$GITHUB_REPOSITORY" --workflow release.yml --event workflow_dispatch --status completed --limit 1000 --json databaseId --jq '.[].databaseId')
while IFS= read -r run; do
  [ -n "$run" ] || continue
  rm -f "$dir/release-source.json"
  gh run download "$run" --repo "$GITHUB_REPOSITORY" --name release-source --dir "$dir" 2>/dev/null || continue
  if jq -e --arg tag "$tag" --arg sha "$sha" '.tag == $tag and .sha == $sha' "$dir/release-source.json" >/dev/null &&
    gh run view "$run" --repo "$GITHUB_REPOSITORY" --json jobs | jq -e '
      [.jobs[] | select(.conclusion == "success") | .name] as $passed |
      ($passed | index("host")) != null or
      (["plan", "build-ui", "build-host / build"] - $passed | length == 0)
    ' >/dev/null; then
    echo "$run"
    exit 0
  fi
done <<< "$runs"

echo "::error::No successful release artifact build with retained source metadata found for $tag ($sha)." >&2
exit 1
