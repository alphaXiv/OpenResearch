"""CLI tests for ScholarCatalyst's offline evaluation contract."""

import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = Path(__file__).with_name("scholarcatalyst_eval.py")


def dump_jsonl(path, rows):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(row) + "\n" for row in rows), encoding="utf-8")


class ScholarCatalystCliTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.bench = self.root / "bench"
        self.bench.mkdir()
        dump_jsonl(self.bench / "corpus.jsonl", [
            {"id": "arxiv_2301.00001", "title": "A Long Example Research Paper", "published": "2023-01-03"},
            {"id": "source-alias", "title": "A Long Example Research Paper", "published": "2023-01-03"},
            {"id": "d1", "title": "Positive one", "published": "2022-12-15"},
            {"id": "d2", "title": "Positive two", "published": "2023-01"},
            {"id": "d3", "title": "Negative", "published": "2022"},
            {"id": "future", "title": "Future", "published": "2023-02-01"},
            {"id": "unknown-date", "title": "Unknown date", "published": ""},
        ])
        self.queries = [
            {"id": "q1", "type": "core_query", "question": "Find related work", "paper_id": "arxiv_2301.00001", "paper_published": "2023-01-03"},
            {"id": "q2", "type": "core_query", "question": "Find another answer", "paper_id": "missing-source", "paper_published": "2023"},
            {"id": "q-empty", "type": "core_query", "question": "No positive labels", "paper_id": "missing-source", "paper_published": "2023"},
            {"id": "q3", "type": "subfield_query", "question": "Find subfield work", "paper_id": "missing-source", "paper_published": ""},
        ]
        dump_jsonl(self.bench / "queries.jsonl", self.queries)
        dump_jsonl(self.bench / "rels" / "core_query.jsonl", [
            {"query_id": "q1", "positive_docs": ["d1", "d2"], "hard_negatives": ["d3"]},
            {"query_id": "q2", "positive_docs": ["d2"], "hard_negatives": ["d3"]},
            {"query_id": "q-empty", "positive_docs": [], "hard_negatives": ["d3"]},
        ])
        dump_jsonl(self.bench / "rels" / "subfield_query.jsonl", [
            {"query_id": "q3", "positive_docs": ["d3"], "hard_negatives": []},
        ])

    def tearDown(self):
        self.temp.cleanup()

    def run_cli(self, *args):
        return subprocess.run([sys.executable, str(SCRIPT), *map(str, args)],
                              text=True, capture_output=True, check=False)

    def test_prepare_and_evaluate_exact_metrics_and_missing_query(self):
        tasks = self.root / "tasks.jsonl"
        prepared = self.run_cli("prepare", "--bench-dir", self.bench,
                                "--query-type", "core_query", "--output", tasks)
        self.assertEqual(prepared.returncode, 0, prepared.stderr)
        task_rows = [json.loads(line) for line in tasks.read_text().splitlines()]
        self.assertEqual(len(task_rows), 3)
        self.assertEqual(set(task_rows[0]), {"id", "type", "question", "cutoff"})
        self.assertEqual(task_rows[0]["cutoff"], "2023-01-03")
        self.assertTrue(task_rows[0]["id"].startswith("task_"))
        self.assertNotIn("q1", tasks.read_text())
        self.assertNotIn("2301.00001", tasks.read_text())
        self.assertFalse(any("paper_id" in row or "positive_docs" in row for row in task_rows))

        run_path = self.root / "run.jsonl"
        # d1 and d2 are found below one negative; q2 has no run row.
        dump_jsonl(run_path, [{"query_id": "q1", "ranking": [
            {"doc_id": "d3", "score": 0.9}, {"doc_id": "d1", "score": 0.8},
            {"doc_id": "d2", "score": 0.7}], "seen": ["d1", "d2", "d3"]}])
        report_path = self.root / "report.json"
        result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", run_path,
                              "--query-type", "core_query", "--query-ids", tasks,
                              "--output", report_path)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(report_path.read_text())
        metrics = report["metrics"]["core_query"]
        self.assertEqual(report["protocol"], "offline_replay")
        self.assertEqual(report["counts"]["missing_run_rows"], 2)
        self.assertEqual(metrics["n_queries"], 3)
        self.assertEqual(metrics["n_scored"], 2)
        self.assertEqual(metrics["n_no_positive"], 1)
        self.assertEqual(metrics["missing_run_queries"], 2)
        self.assertEqual(metrics["n_seen"], 1)
        self.assertEqual(metrics["candidate_trajectory_recall"], 1.0)
        self.assertEqual(metrics["recall@5"], 0.5)
        self.assertEqual(metrics["recall@20"], 0.5)
        self.assertEqual(metrics["recall@100"], 0.5)
        expected_query_ndcg = (1.0 / math.log2(3) + 1.0 / math.log2(4)) / (1.0 + 1.0 / math.log2(3))
        expected_ndcg = expected_query_ndcg / 2
        self.assertAlmostEqual(metrics["ndcg@20"], expected_ndcg, places=6)
        self.assertEqual(metrics["ranking_depth_counts"]["0"], 2)
        self.assertEqual(metrics["source_title_unavailable"], 2)
        self.assertIn("run", report["inputs_sha256"])

    def test_limit_must_be_positive(self):
        result = self.run_cli("prepare", "--bench-dir", self.bench, "--output", self.root / "x",
                              "--limit", "0")
        self.assertEqual(result.returncode, 2)
        self.assertIn("positive integer", result.stderr)

    def test_reject_duplicate_run_query(self):
        path = self.root / "duplicate.jsonl"
        dump_jsonl(path, [{"query_id": "q1", "ranking": []}, {"query_id": "q1", "ranking": []}])
        result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", path,
                              "--query-type", "core_query", "--output", self.root / "out.json")
        self.assertEqual(result.returncode, 2)
        self.assertIn("duplicate run query id", result.stderr)

    def test_reject_duplicate_relation_query_and_wrong_types(self):
        path = self.bench / "rels" / "core_query.jsonl"
        row = {"query_id": "q1", "positive_docs": ["d1"], "hard_negatives": []}
        dump_jsonl(path, [row, row])
        result = self.run_cli("prepare", "--bench-dir", self.bench, "--output", self.root / "tasks.jsonl",
                              "--query-type", "core_query")
        self.assertEqual(result.returncode, 2)
        self.assertIn("duplicate relation query id", result.stderr)
        dump_jsonl(path, [{"query_id": "q1", "positive_docs": "d1", "hard_negatives": []},
                          {"query_id": "q2", "positive_docs": ["d2"], "hard_negatives": []},
                          {"query_id": "q-empty", "positive_docs": [], "hard_negatives": []}])
        result = self.run_cli("prepare", "--bench-dir", self.bench, "--output", self.root / "tasks.jsonl",
                              "--query-type", "core_query")
        self.assertEqual(result.returncode, 2)
        self.assertIn("must be lists", result.stderr)

    def test_reject_duplicate_ranking_unknown_id_and_future_document(self):
        for ranking, expected in [
            ([{"doc_id": "d1"}, {"doc_id": "d1"}], "duplicate ranked document"),
            ([{"doc_id": "not-in-corpus"}], "unknown corpus document"),
            ([{"doc_id": "future"}], "post-cutoff document"),
        ]:
            with self.subTest(expected=expected):
                path = self.root / "bad-run.jsonl"
                dump_jsonl(path, [{"query_id": "q1", "ranking": ranking}])
                result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", path,
                                      "--query-type", "core_query", "--output", self.root / "out.json")
                self.assertEqual(result.returncode, 2)
                self.assertIn(expected, result.stderr)

    def test_reject_ranking_outside_seen(self):
        path = self.root / "bad-seen.jsonl"
        dump_jsonl(path, [{"query_id": "q1", "ranking": [{"doc_id": "d1"}], "seen": []}])
        result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", path,
                              "--query-type", "core_query", "--output", self.root / "out.json")
        self.assertEqual(result.returncode, 2)
        self.assertIn("outside seen", result.stderr)

    def test_reject_source_paper_and_title_alias_in_ranking_or_seen(self):
        cases = [
            ({"query_id": "q1", "ranking": [{"doc_id": "arxiv_2301.00001"}]}, "ranking includes"),
            ({"query_id": "q1", "ranking": [], "seen": ["source-alias"]}, "seen includes"),
        ]
        for row, message in cases:
            with self.subTest(message=message):
                path = self.root / "source-leak.jsonl"
                dump_jsonl(path, [row])
                result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", path,
                                      "--query-type", "core_query", "--output", self.root / "out.json")
                self.assertEqual(result.returncode, 2)
                self.assertIn(message, result.stderr)

    def test_reject_missing_relation_file_and_malformed_json(self):
        (self.bench / "rels" / "subfield_query.jsonl").unlink()
        result = self.run_cli("prepare", "--bench-dir", self.bench, "--output", self.root / "tasks.jsonl")
        self.assertEqual(result.returncode, 2)
        self.assertIn("required relation file", result.stderr)

    def test_reject_malformed_run_and_output_overwrite(self):
        malformed = self.root / "malformed.jsonl"
        malformed.write_text("{broken\n", encoding="utf-8")
        result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", malformed,
                              "--query-type", "core_query", "--output", self.root / "out.json")
        self.assertEqual(result.returncode, 2)
        self.assertIn("invalid JSON", result.stderr)
        valid = self.root / "valid.jsonl"
        dump_jsonl(valid, [])
        original = valid.read_text(encoding="utf-8")
        result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", valid,
                              "--query-type", "core_query", "--output", valid)
        self.assertEqual(result.returncode, 2)
        self.assertIn("must not overwrite an input file", result.stderr)
        self.assertEqual(valid.read_text(encoding="utf-8"), original)

    def test_missing_relation_row_is_error(self):
        path = self.bench / "rels" / "core_query.jsonl"
        dump_jsonl(path, [{"query_id": "q1", "positive_docs": [], "hard_negatives": []}])
        result = self.run_cli("prepare", "--bench-dir", self.bench, "--output", self.root / "tasks.jsonl",
                              "--query-type", "core_query")
        self.assertEqual(result.returncode, 2)
        self.assertIn("no relation row", result.stderr)

    def test_short_title_does_not_create_alias(self):
        query_path = self.bench / "queries.jsonl"
        rows = [dict(row) for row in self.queries]
        rows[0]["paper_title"] = "Short Title"
        rows[0]["paper_id"] = "missing-explicit-source"
        dump_jsonl(query_path, rows)
        path = self.root / "short-title.jsonl"
        dump_jsonl(path, [{"query_id": "q1", "ranking": [
            {"doc_id": "source-alias"}, {"doc_id": "d1"}, {"doc_id": "d2"}]}])
        result = self.run_cli("evaluate", "--bench-dir", self.bench, "--run", path,
                              "--query-type", "core_query", "--output", self.root / "out.json")
        self.assertEqual(result.returncode, 0, result.stderr)
        metrics = json.loads((self.root / "out.json").read_text())["metrics"]["core_query"]
        self.assertEqual(metrics["per_query"][0]["ranking_depth"], 3)


if __name__ == "__main__":
    unittest.main()
