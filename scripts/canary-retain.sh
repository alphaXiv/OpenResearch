#!/usr/bin/env bash
set -euo pipefail
mkdir -p "$RUNNER_TEMP/canary-retention"
gh release download canary --repo "$GITHUB_REPOSITORY" --pattern desktop-canary-v2.json --dir "$RUNNER_TEMP/canary-retention"
latest=$(jq -er '.tag' "$RUNNER_TEMP/canary-retention/desktop-canary-v2.json")
release_id=$(gh api "repos/$GITHUB_REPOSITORY/releases/tags/canary" --jq .id)
gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases/$release_id/assets?per_page=100" | jq '{assets: [.[][]]}' > "$RUNNER_TEMP/canary-retention/assets.json"
jq -r --arg latest "$latest" '
  [.assets[] | select(.name | test("^canary-[0-9]{8}\\.[0-9]+--OpenResearch"))
   | {id, tag: (.name | capture("^(?<tag>canary-[0-9]{8}\\.[0-9]+)--").tag)}]
  | group_by(.tag)
  | sort_by(.[0].tag | capture("^canary-(?<date>[0-9]{8})\\.(?<sequence>[0-9]+)$") | [.date, (.sequence | tonumber)])
  | reverse | if any(.[]; .[0].tag == $latest) then map(select(.[0].tag != $latest)) | .[6:] else .[7:] end
  | .[] | .[].id
' "$RUNNER_TEMP/canary-retention/assets.json" > "$RUNNER_TEMP/canary-retention/obsolete-assets.txt"
while IFS= read -r id; do
  gh api -X DELETE "repos/$GITHUB_REPOSITORY/releases/assets/$id" > /dev/null
done < "$RUNNER_TEMP/canary-retention/obsolete-assets.txt"
builds=$(jq '[.assets[].name | select(test("^canary-[0-9]{8}\\.[0-9]+--OpenResearch")) | capture("^(?<tag>canary-[0-9]{8}\\.[0-9]+)--").tag] | unique | length' "$RUNNER_TEMP/canary-retention/assets.json")
if [ "$builds" -ge 7 ]; then
  gh api --paginate --slurp "repos/$GITHUB_REPOSITORY/releases?per_page=100" | jq -r --arg latest "$latest" '
    .[][] | select(.draft == false and .prerelease == true and .tag_name != $latest and (.tag_name | test("^canary-[0-9]{8}\\.[0-9]+$"))) | .tag_name
  ' > "$RUNNER_TEMP/canary-retention/obsolete-releases.txt"
  while IFS= read -r tag; do
    gh release delete "$tag" --repo "$GITHUB_REPOSITORY" --yes --cleanup-tag
  done < "$RUNNER_TEMP/canary-retention/obsolete-releases.txt"
fi
