#!/usr/bin/env bash
set -euo pipefail
if ! gh release view canary --repo "$GITHUB_REPOSITORY" >/dev/null 2>&1; then
  gh release create canary --repo "$GITHUB_REPOSITORY" --target "$SOURCE_SHA" --prerelease --latest=false --title Canary --notes 'Latest desktop canary builds.'
fi
release_id=$(gh api "repos/$GITHUB_REPOSITORY/releases/tags/canary" --jq .id)
existing=$(gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases/$release_id/assets?per_page=100" | jq -r '.[][].name')
if grep -Fxq desktop-canary-v2.json <<< "$existing"; then
  mkdir -p "$RUNNER_TEMP/previous-canary"
  gh release download canary --repo "$GITHUB_REPOSITORY" --pattern desktop-canary-v2.json --dir "$RUNNER_TEMP/previous-canary"
  previous=$(jq -er '.commit | select(type == "string" and test("^[0-9a-f]{40}$"))' "$RUNNER_TEMP/previous-canary/desktop-canary-v2.json")
  if git merge-base --is-ancestor "$previous" "$SOURCE_SHA"; then
    :
  else
    result=$?
    if [ "$result" -eq 1 ]; then
      echo "Keeping the newer canary from $previous."
      exit 0
    fi
    exit "$result"
  fi
fi
canary_commit=$(gh api "repos/$GITHUB_REPOSITORY/commits/canary" --jq .sha)
git merge-base --is-ancestor "$canary_commit" "$SOURCE_SHA"
git rev-list "$SOURCE_SHA" > "$RUNNER_TEMP/canary-ancestry.txt"

stable=$(gh api "repos/$GITHUB_REPOSITORY/releases/latest" --jq .tag_name)
notes=
if git merge-base --is-ancestor "$stable" "$SOURCE_SHA"; then
  notes=$(git log --no-merges -n 300 --format='%H%x09%s' "$stable..$SOURCE_SHA" | jq -R -s -c --arg base "$stable" '{base: $base, commits: (split("\n")[:-1] | map(split("\t") | {sha: .[0], title: .[1]}))}')
fi

mkdir -p "$RUNNER_TEMP/shared-canary" "$RUNNER_TEMP/published-canary"
for asset in artifacts/OpenResearch*; do
  name="$TAG--${asset##*/}"
  if ! grep -Fxq "$name" <<< "$existing"; then
    cp "$asset" "$RUNNER_TEMP/shared-canary/$name"
    gh release upload canary "$RUNNER_TEMP/shared-canary/$name" --repo "$GITHUB_REPOSITORY"
  fi
done
gh release download canary --repo "$GITHUB_REPOSITORY" --pattern "$TAG--OpenResearch*" --dir "$RUNNER_TEMP/published-canary"
CANARY_NOTES="$notes" node "$(dirname "$0")/canary-manifest.mjs" "$RUNNER_TEMP/published-canary" "$VERSION" "$TAG" "$SOURCE_SHA" "$RUNNER_TEMP/canary-ancestry.txt" > artifacts/desktop-canary-v2.json
gh release upload canary artifacts/desktop-canary-v2.json --repo "$GITHUB_REPOSITORY" --clobber
{
  printf 'Latest build: %s\nCommit: %s\n' "$TAG" "$SOURCE_SHA"
  if [ -n "$notes" ]; then
    jq -r '"\n## Changes since \(.base)\n", (.commits[] | "- \(.title)")' <<< "$notes"
  fi
} > "$RUNNER_TEMP/canary-notes.txt"
gh release edit canary --repo "$GITHUB_REPOSITORY" --title Canary --notes-file "$RUNNER_TEMP/canary-notes.txt"
if ! gh api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/tags/canary" -f sha="$SOURCE_SHA" -F force=false > /dev/null; then
  echo '::warning::Could not move the Canary tag; the manifest and release notes identify the latest build.'
fi
