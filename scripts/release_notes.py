#!/usr/bin/env python3
"""Combine GitHub's change list, linked issues, optional highlights, and dist's install notes."""

import json
import os
import re
import subprocess
import sys
from pathlib import Path


ISSUES_QUERY = """query($owner: String!, $repo: String!, $number: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $number) {
      closingIssuesReferences(first: 100) { nodes { title url state } }
    }
  }
}"""


def gh(*args):
    return json.loads(subprocess.check_output(["gh", "api", *args], text=True))


def fixed_issues(notes, repository):
    owner, repo = repository.split("/", 1)
    pattern = rf"https://github\.com/{re.escape(repository)}/pull/(\d+)"
    issues = []
    seen = set()
    for number in dict.fromkeys(re.findall(pattern, notes)):
        try:
            result = gh(
                "graphql", "-f", f"query={ISSUES_QUERY}", "-f", f"owner={owner}",
                "-f", f"repo={repo}", "-F", f"number={number}",
            )
            linked = result["data"]["repository"]["pullRequest"]["closingIssuesReferences"]["nodes"]
        except (subprocess.CalledProcessError, KeyError, TypeError, ValueError) as error:
            print(f"::warning::Could not list fixed issues for PR #{number}: {error}", file=sys.stderr)
            continue
        for issue in linked:
            if issue["state"] == "CLOSED" and issue["url"] not in seen:
                seen.add(issue["url"])
                title = issue["title"].replace("\\", "\\\\").replace("[", "\\[").replace("]", "\\]")
                issues.append(
                    f"- [{title}]({issue['url']}) — fixed by "
                    f"[#{number}](https://github.com/{repository}/pull/{number})"
                )
    return issues


def compose(notes, issues, highlights, installation):
    notes = notes.strip()
    if issues:
        section = "## Fixed issues\n\n" + "\n".join(issues)
        marker = "**Full Changelog**:"
        before, found, after = notes.partition(marker)
        notes = (before.rstrip() + "\n\n" + section + "\n\n" + found + after) if found else notes + "\n\n" + section
    return "\n\n".join(part for part in (highlights.strip(), notes, installation.strip()) if part) + "\n"


def main():
    tag, commit, installation_file, output_file = sys.argv[1:]
    repository = os.environ["GITHUB_REPOSITORY"]
    generated = gh(
        f"repos/{repository}/releases/generate-notes", "-X", "POST",
        "-f", f"tag_name={tag}", "-f", f"target_commitish={commit}",
    )["body"]
    highlights_file = Path("release-notes") / f"{tag}.md"
    highlights = highlights_file.read_text() if highlights_file.is_file() else ""
    installation = Path(installation_file).read_text()
    Path(output_file).write_text(compose(generated, fixed_issues(generated, repository), highlights, installation))


if __name__ == "__main__":
    main()
