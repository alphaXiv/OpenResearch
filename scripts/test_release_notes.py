import unittest
import subprocess
import os
from unittest.mock import patch

from release_notes import compose, fixed_issues, refresh_downloads, DESKTOP_ASSETS, main, remote_install


class ReleaseNotesTest(unittest.TestCase):
    def test_linked_issues_and_order(self):
        notes = (
            "## What's Changed\n"
            "* Fix paper in https://github.com/alphaXiv/OpenResearch/pull/417\n"
            "* Follow-up in https://github.com/alphaXiv/OpenResearch/pull/418\n\n"
            "**Full Changelog**: https://github.com/alphaXiv/OpenResearch/compare/v1...v2\n"
        )
        issue = {"title": "Handle [arXiv] links", "url": "https://github.com/alphaXiv/OpenResearch/issues/457", "state": "CLOSED"}
        open_issue = {"title": "Still open", "url": "https://github.com/alphaXiv/OpenResearch/issues/999", "state": "OPEN"}
        response = {"data": {"repository": {"pullRequest": {"closingIssuesReferences": {"nodes": [issue, open_issue]}}}}}
        with patch("release_notes.gh", return_value=response) as api:
            issues = fixed_issues(notes, "alphaXiv/OpenResearch")
        self.assertEqual(api.call_count, 2)
        self.assertEqual(len(issues), 1)
        rendered = compose(notes, issues, "## Highlights\n\nCustom text.", "")
        self.assertLess(rendered.index("## Highlights"), rendered.index("## What's Changed"))
        self.assertLess(rendered.index("## What's Changed"), rendered.index("## Fixed issues"))
        self.assertLess(rendered.index("## Fixed issues"), rendered.index("**Full Changelog**"))
        self.assertLess(rendered.index("## Download OpenResearch"), rendered.index("## Highlights"))
        self.assertIn("[Handle \\[arXiv\\] links]", rendered)
        self.assertNotIn("Still open", rendered)
        self.assertNotIn("## Highlights", compose(notes, [], "", ""))

    def test_desktop_downloads_only_link_uploaded_assets(self):
        assets = [
            {"name": name, "browser_download_url": f"https://github.com/alphaXiv/OpenResearch/releases/download/v0.2.14/{name}"}
            for name, _, _ in DESKTOP_ASSETS
        ]
        assets.append({"name": "openresearch-cli-installer.sh", "browser_download_url": "https://example.com/installer.sh"})
        body = compose("## What's Changed\n\nChanges.", [], "## Highlights\n\nHighlights.", "")
        self.assertIn("https://openresearch.sh/", body)
        self.assertNotIn("openresearch-cli", body)
        partial = refresh_downloads(body, assets[:1])
        self.assertIn("OpenResearch.dmg", partial)
        self.assertNotIn("OpenResearch-Setup.exe", partial)
        refreshed = refresh_downloads(partial, assets)
        for name, _, _ in DESKTOP_ASSETS:
            self.assertIn(name, refreshed)
        self.assertEqual(refreshed.count('<picture>'), 4)
        self.assertIn('download-macos-dark.svg', refreshed)
        self.assertIn('alt="Download for Linux (x86)"', refreshed)
        self.assertIn('download-linux-arm64-dark.svg', refreshed)
        self.assertIn('alt="Download for Linux (ARM64)"', refreshed)
        self.assertNotIn("installer.sh", refreshed)
        self.assertNotIn("https://openresearch.sh/", refreshed)
        self.assertTrue(refreshed.endswith("## Highlights\n\nHighlights.\n\n## What's Changed\n\nChanges.\n"))
        self.assertEqual(refresh_downloads(refreshed, assets), refreshed)
        legacy = "Older notes without a managed section"
        self.assertEqual(refresh_downloads(legacy, assets), legacy)
        for malformed in ("<!-- desktop-downloads:start -->", "<!-- desktop-downloads:end -->"):
            with self.assertRaises(ValueError):
                refresh_downloads(malformed, assets)

    def test_refresh_patches_only_the_selected_release_body(self):
        body = compose("## What's Changed\n\nChanges.", [], "", "")
        assets = [{"name": "OpenResearch.dmg", "browser_download_url": "https://example.com/OpenResearch.dmg"}]
        release = {"id": 123, "body": body, "assets": assets}
        with patch.dict(os.environ, {"GITHUB_REPOSITORY": "alphaXiv/OpenResearch"}), \
                patch("sys.argv", ["release_notes.py", "--refresh-downloads", "v0.2.14"]), \
                patch("release_notes.gh", side_effect=[release, {}]) as api:
            main()
        self.assertEqual(api.call_args_list[0].args, ("repos/alphaXiv/OpenResearch/releases/tags/v0.2.14",))
        self.assertEqual(api.call_args_list[1].args, (
            "repos/alphaXiv/OpenResearch/releases/123", "-X", "PATCH", "-f",
            "body=" + refresh_downloads(body, assets),
        ))

    def test_refresh_does_not_patch_legacy_or_unchanged_notes(self):
        for body in ("Older release notes", compose("Changes.", [], "", "")):
            with patch.dict(os.environ, {"GITHUB_REPOSITORY": "alphaXiv/OpenResearch"}), \
                    patch("sys.argv", ["release_notes.py", "--refresh-downloads", "v0.2.14"]), \
                    patch("release_notes.gh", return_value={"id": 123, "body": body, "assets": []}) as api:
                main()
            self.assertEqual(api.call_count, 1)

    def test_remote_install_is_collapsed_and_versioned(self):
        remote = remote_install("alphaXiv/OpenResearch", "v0.2.14")
        body = compose("## What's Changed\n\nChanges.", [], "", remote)
        self.assertIn("<details>\n<summary>Install ORX on a remote machine</summary>", body)
        self.assertIn("curl --proto '=https' --tlsv1.2 -LsSf https://github.com/alphaXiv/OpenResearch/releases/download/v0.2.14/openresearch-cli-installer.sh | sh", body)
        self.assertLess(body.index("## Download OpenResearch"), body.index("<details>"))
        self.assertLess(body.index("</details>"), body.index("## What's Changed"))
        self.assertIn(remote, refresh_downloads(body, []))

    def test_issue_lookup_failure_keeps_other_issues(self):
        notes = (
            "* First in https://github.com/alphaXiv/OpenResearch/pull/1\n"
            "* Second in https://github.com/alphaXiv/OpenResearch/pull/2"
        )
        issue = {"title": "Fixed", "url": "https://github.com/alphaXiv/OpenResearch/issues/3", "state": "CLOSED"}
        response = {"data": {"repository": {"pullRequest": {"closingIssuesReferences": {"nodes": [issue]}}}}}
        with patch("release_notes.gh", side_effect=[subprocess.CalledProcessError(1, "gh"), response]):
            issues = fixed_issues(notes, "alphaXiv/OpenResearch")
        self.assertEqual(len(issues), 1)


if __name__ == "__main__":
    unittest.main()
