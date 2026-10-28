"""Generate the onboarding figure from the historical pipeline's recorded evidence."""
import csv
import json
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT.parents[1] / "agent-skills/orx-figures/assets"))
from orx_chart import render_chart


rows = list(csv.DictReader((ROOT / "evidence/training-metrics.csv").open()))
evaluation = json.loads((ROOT / "evidence/evaluation-metrics.json").read_text())
views = []
for phase, name in [("base", "Base training"), ("sft", "Supervised fine-tuning")]:
    points = [dict(step=int(row["step"]), bpb=float(row["validation_bpb"]))
              for row in rows if row["phase"] == phase and row["validation_bpb"]]
    assert points[-1]["bpb"] == evaluation["final"][f"{phase}ValidationBpb"]
    views.append(dict(key="bpb" if phase == "base" else "sftBpb",
                      label=f"{name} · validation BPB", yLabel="Validation", unit="BPB",
                      series=[dict(name=name, points=[dict(step=p["step"], **{
                          "bpb" if phase == "base" else "sftBpb": p["bpb"]}) for p in points])]))

labels = {"bigbench_qa_wikidata": "Wikidata", "openbook_qa": "OpenBookQA",
          "winogrande": "Winogrande", "bigbench_operators": "Operators"}
views.append(dict(key="accuracy", label="CORE benchmark accuracy", type="bar",
                  xLabel="Benchmark", yLabel="Accuracy", unit="%",
                  series=[dict(name="Base model", points=[
                      dict(step=i, label=labels[row["task"]], accuracy=100 * row["accuracy"])
                      for i, row in enumerate(evaluation["core"])])]))
render_chart(ROOT / "figures/nanochat-pipeline-results.html",
             title="Nanochat · CPU / Apple Silicon pipeline",
             subtitle="Recorded demo run · 73.5M parameters · base training + SFT",
             metrics=views)
