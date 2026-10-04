#!/usr/bin/env python3
"""Compose desktop-first release notes and refresh published download links."""

import json
from html import escape
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


DOWNLOAD_START = "<!-- desktop-downloads:start -->"
DOWNLOAD_END = "<!-- desktop-downloads:end -->"
DESKTOP_ASSETS = (
    ("OpenResearch.dmg", "Download for macOS — Apple silicon and Intel", "macos"),
    ("OpenResearch-Setup.exe", "Download for Windows", "windows"),
    ("OpenResearch-x86_64.AppImage", "Download for Linux (x86)", "linux-x86"),
    ("OpenResearch-aarch64.AppImage", "Download for Linux (ARM64)", "linux-arm64"),
)


def downloads(assets=()):
    urls = {asset["name"]: asset["browser_download_url"] for asset in assets}
    images = "https://raw.githubusercontent.com/alphaXiv/OpenResearch/main/.github/readme-assets"
    buttons = [
        f'<a href="{escape(urls[name], quote=True)}"><picture>'
        f'<source media="(prefers-color-scheme: dark)" srcset="{images}/download-{platform}-dark.svg">'
        f'<img src="{images}/download-{platform}.svg" alt="{label}" width="220" height="44" />'
        '</picture></a>'
        for name, label, platform in DESKTOP_ASSETS if name in urls
    ]
    links = ["<p>\n" + "\n".join(buttons) + "\n</p>"] if buttons else []
    if not links:
        links = ["[Download the desktop app](https://openresearch.sh/)"]
    return "\n\n".join((
        DOWNLOAD_START,
        "## Download OpenResearch",
        "Install the desktop app to get everything you need. No separate installation or terminal setup required.",
        "\n\n".join(links),
        "The desktop app updates automatically by default. You can manage updates in Settings.",
        DOWNLOAD_END,
    ))


def refresh_downloads(body, assets):
    before, start, rest = body.partition(DOWNLOAD_START)
    _, end, after = rest.partition(DOWNLOAD_END)
    if not start and DOWNLOAD_END not in body:
        return body
    if not start or not end:
        raise ValueError("Release has no managed desktop download section")
    return before + downloads(assets) + after


def remote_install(repository, tag):
    url = f"https://github.com/{repository}/releases/download/{tag}/openresearch-cli-installer.sh"
    return (
        "<details>\n<summary>Install ORX on a remote machine</summary>\n\n"
        "For remote servers and compute clusters, install ORX directly from a terminal:\n\n"
        "```sh\n"
        f"curl --proto '=https' --tlsv1.2 -LsSf {url} | sh\n"
        "```\n\n"
        "To update an existing terminal installation, run `orx update`.\n\n"
        "</details>"
    )


def compose(notes, issues, highlights, remote):
    notes = notes.strip()
    if issues:
        section = "## Fixed issues\n\n" + "\n".join(issues)
        marker = "**Full Changelog**:"
        before, found, after = notes.partition(marker)
        notes = (before.rstrip() + "\n\n" + section + "\n\n" + found + after) if found else notes + "\n\n" + section
    return "\n\n".join(part for part in (downloads(), remote.strip(), highlights.strip(), notes) if part) + "\n"


def main():
    repository = os.environ["GITHUB_REPOSITORY"]
    if sys.argv[1] == "--refresh-downloads":
        tag = sys.argv[2]
        release = gh(f"repos/{repository}/releases/tags/{tag}")
        body = refresh_downloads(release["body"], release["assets"])
        if body == release["body"]:
            print("Desktop download section is unchanged; skipping refresh.")
            return
        gh(f"repos/{repository}/releases/{release['id']}", "-X", "PATCH", "-f", f"body={body}")
        return
    tag, commit, output_file = sys.argv[1:]
    generated = gh(
        f"repos/{repository}/releases/generate-notes", "-X", "POST",
        "-f", f"tag_name={tag}", "-f", f"target_commitish={commit}",
    )["body"]
    highlights_file = Path("release-notes") / f"{tag}.md"
    highlights = highlights_file.read_text() if highlights_file.is_file() else ""
    Path(output_file).write_text(compose(generated, fixed_issues(generated, repository), highlights, remote_install(repository, tag)))


if __name__ == "__main__":
    main()
