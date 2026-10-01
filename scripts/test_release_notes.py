import unittest
import subprocess
from unittest.mock import patch

from release_notes import compose, fixed_issues


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
        rendered = compose(notes, issues, "## Highlights\n\nCustom text.", "## Install\n\nCommand")
        self.assertLess(rendered.index("## Highlights"), rendered.index("## What's Changed"))
        self.assertLess(rendered.index("## What's Changed"), rendered.index("## Fixed issues"))
        self.assertLess(rendered.index("## Fixed issues"), rendered.index("**Full Changelog**"))
        self.assertLess(rendered.index("**Full Changelog**"), rendered.index("## Install"))
        self.assertIn("[Handle \\[arXiv\\] links]", rendered)
        self.assertNotIn("Still open", rendered)
        self.assertNotIn("## Highlights", compose(notes, [], "", "## Install"))

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
