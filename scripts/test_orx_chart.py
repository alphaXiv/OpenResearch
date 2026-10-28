"""Regression checks for the offline chart authoring contract."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ASSETS = Path(__file__).resolve().parents[1] / "agent-skills/orx-figures/assets"
spec = importlib.util.spec_from_file_location("orx_chart", ASSETS / "orx_chart.py")
chart = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chart)


class ChartTests(unittest.TestCase):
    def test_numeric_templates_accept_measurements_and_missing_checkpoints(self):
        for kind in chart.CHART_TYPES - {"diagram"}:
            with self.subTest(kind=kind):
                chart.validate_views([{ "key": "loss", "type": kind }], [
                    {"name": "run", "points": [{"step": 1, "loss": 2}, {"step": 2, "loss": None}]}
                ])

    def test_rejects_invalid_views_and_measurements(self):
        for metric, point in [
            ({"key": "loss", "type": "bar", "yScale": "log"}, {"step": 1, "loss": 2}),
            ({"key": "loss", "type": "area", "yScale": "log"}, {"step": 1, "loss": 2}),
            ({"key": "loss", "type": "unknown"}, {"step": 1, "loss": 2}),
            ({"key": "loss"}, {"step": 1, "loss": float("inf")}),
            ({"key": "loss", "type": "scaling"}, {"step": 0, "loss": 2}),
            ({"key": "loss"}, {"step": 1, "loss": 2, "lower": 3, "upper": 4}),
            ({"key": "loss", "yScale": "log"}, {"step": 1, "loss": 2, "lower": 0, "upper": 4}),
            ({"key": "loss"}, {"step": 1, "loss": 2, "lower": float("nan"), "upper": 4}),
        ]:
            with self.subTest(metric=metric, point=point), self.assertRaises(ValueError):
                chart.validate_views([metric], [{"name": "run", "points": [point]}])

    def test_diagram_edges_require_existing_nodes(self):
        metric = {"key": "method", "type": "diagram", "nodes": [{"id": "a", "column": 0, "row": 0}], "edges": [{"from": "a", "to": "b"}]}
        with self.assertRaises(ValueError):
            chart.validate_views([metric], [])
        metric["edges"] = []
        chart.validate_views([metric], [])

    def test_html_payload_cannot_break_out_of_data_script(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / "chart.html"
            title = "</script><script>alert(1)</script>"
            chart.render_chart(output, title=title, subtitle="probe", metrics=[{"key": "loss", "label": "Loss"}], series=[{"name": "run", "points": [{"step": 0, "loss": 2}]}])
            html = output.read_text()
            self.assertNotIn(title, html)
            payload = html.split('<script id="data" type="application/json">')[1].split('</script>')[0]
            self.assertEqual(json.loads(payload)["title"], title)
            self.assertNotIn("__ORX_CHART_DATA__", html)


if __name__ == "__main__":
    unittest.main()
