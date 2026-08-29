---
name: orx-reports
description: "Write durable outputs into the artifacts directory. Use when a line of work concludes or the user asks for a write-up, summary, comparison, figures, or exported data."
---

Write reports, figures, CSVs, PDFs, and other outputs directly into the artifacts
directory shown in the session playbook. Written files become project artifacts
immediately.

When a line of work concludes, use a descriptive filename that explains the
output without relying on its directory, for example:

- `<artifacts-dir>/scaling-analysis.md`
- `<artifacts-dir>/benchmark-results.csv`
- `<artifacts-dir>/ablation-comparison.png`

Read the `orx-figures` module before writing any figure; a default
matplotlib plot does not meet the bar the reports are held to.

Write at the artifacts root unless a folder is useful. Markdown may reference
nearby images by relative path; no name such as `project/`, an experiment slug,
or `report.md` is reserved. In the chat handoff, link every finished output using
the session playbook's evidence-and-links contract. Load `orx-evidence` when the
report makes claims derived from run results.
