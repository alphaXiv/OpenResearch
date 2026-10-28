"""Render offline interactive metrics using the adjacent OpenResearch template."""
import json
import math
from pathlib import Path


CHART_TYPES = {"line", "area", "scatter", "scaling", "bar", "dot", "pareto", "heatmap", "confusion", "diagram"}


def validate_views(metrics, series):
    keys = [metric["key"] for metric in metrics]
    if not keys or len(keys) != len(set(keys)):
        raise ValueError("Graph views require unique keys")
    for metric in metrics:
        kind = metric.get("type", "line")
        if kind not in CHART_TYPES:
            raise ValueError(f"Unsupported graph template: {kind}")
        if kind == "diagram":
            nodes = metric["nodes"]
            ids = [node["id"] for node in nodes]
            if not ids or len(ids) != len(set(ids)):
                raise ValueError("Diagram nodes require unique IDs")
            for node in nodes:
                if any(not isinstance(node[axis], int) or node[axis] < 0 for axis in ("column", "row")):
                    raise ValueError("Diagram positions require nonnegative integer columns and rows")
            if any(edge["from"] not in ids or edge["to"] not in ids for edge in metric.get("edges", [])):
                raise ValueError("Diagram edges must reference existing nodes")
            continue
        if kind in {"bar", "area"} and metric.get("yScale") == "log":
            raise ValueError("Bar and area charts require a linear y axis")
        runs = metric.get("series", series)
        if not runs:
            raise ValueError("A graph requires at least one series")
        for run in runs:
            measured = [point for point in run["points"] if point.get(metric["key"]) is not None]
            if not measured:
                raise ValueError("Each series requires at least one measurement")
            for point in measured + run.get("fit", []):
                x, y = point["step"], point[metric["key"]]
                if not all(isinstance(value, (int, float)) and math.isfinite(value) for value in (x, y)):
                    raise ValueError("Graph coordinates must be finite numbers")
                if (metric.get("xScale", "log" if kind == "scaling" else "linear") == "log" and x <= 0
                    or metric.get("yScale", "log" if kind == "scaling" else "linear") == "log" and y <= 0):
                    raise ValueError("Log axes require positive coordinates")
                if "lower" in point or "upper" in point:
                    if not all(isinstance(point.get(bound), (int, float)) and math.isfinite(point[bound]) for bound in ("lower", "upper")) or not point["lower"] <= y <= point["upper"]:
                        raise ValueError("Finite intervals must bracket their measurement")
                    if metric.get("yScale", "log" if kind == "scaling" else "linear") == "log" and point["lower"] <= 0:
                        raise ValueError("Log intervals require positive bounds")


def render_chart(output, *, title, subtitle, metrics, series=(), x_label="Training step"):
    validate_views(metrics, series)
    data = dict(title=title, subtitle=subtitle, metrics=metrics, series=series, xLabel=x_label)
    payload = json.dumps(data, allow_nan=False).replace("<", "\\u003c")
    template = Path(__file__).with_name("orx-chart.html").read_text(encoding="utf-8")
    Path(output).write_text(template.replace("__ORX_CHART_DATA__", payload), encoding="utf-8")
