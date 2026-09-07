---
name: orx-reports
description: "Write and organize durable outputs in the artifacts directory. Use before creating or organizing artifacts, including reports, summaries, comparisons, figures, and exported data, or when a line of work concludes."
---

Write reports, figures, CSVs, PDFs, and other outputs directly into the artifacts
directory shown in the session playbook. Written files become project artifacts
immediately.

Before writing, inspect the existing artifacts directory and follow its
organization. Reuse the relevant folder for follow-up outputs. Group related
outputs by research topic or deliverable instead of accumulating files at the
artifacts root. Add subfolders such as `figures/`, `data/`, or `models/` only
when they help; do not create empty scaffolding.

Use a descriptive filename for each output. For example:

```text
<artifacts-dir>/
  transformer-sweep/
    transformer-sweep-report.md
    figures/
      orx_figstyle.py
      patch-size.pdf
      patch-size.svg
      patch-size.py
      learning-rate.pdf
      learning-rate.svg
      learning-rate.py
    data/
      sweep-results.csv
```

Keep reproducibility scripts beside their figures. Keep temporary logs and
scratch files outside the artifacts directory.

Keep already-published outputs at their existing paths. Reorganize other older
outputs only when requested, updating affected document links and verifying they
still resolve.

Read the `orx-figures` module before writing any figure; a default
matplotlib plot does not meet the bar the reports are held to.

Use relative links within reports, for example
`![Patch size](figures/patch-size.svg)` from `transformer-sweep-report.md`.
In the chat handoff, link every finished output using the session playbook's
evidence-and-links contract, including the full nested path, for example
`<file path="artifacts/transformer-sweep/figures/patch-size.svg" />`.
Load `orx-evidence` when the report makes claims derived from run results.
