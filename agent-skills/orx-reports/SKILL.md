---
name: orx-reports
description: "Write and organize durable outputs in the artifacts directory. Use before creating or organizing artifacts, including reports, summaries, and figures."
---

An artifact is a project-wide deliverable that a human researcher will read and analyze. This can include reports, figures, and HTML pages, and more. Written files become project artifacts immediately.

Keep the artifacts directory clean; avoid model checkpoints, rollouts, activations, etc. that are not project-wide that will bloat the artifacts directory.

Use a descriptive filename for each output. For example:

```text
<artifacts-dir>/
  research-summary.md
  literature-review.md
  learning-rate-study/
    learning-rate-study.md
    loss-curves.html
    final-accuracy.svg
```

Read the `orx-figures` module before writing any figure; a default
matplotlib plot does not meet the bar the reports are held to.

Load `orx-results` when the report makes claims derived from run results.

Use relative links within reports, for example
`![Final accuracy](final-accuracy.svg)` from `learning-rate-study.md`.
In the chat handoff, link every finished output using the session playbook's
evidence-and-links contract, including the full path, for example
`<file path="artifacts/learning-rate-study/final-accuracy.svg" />`.
