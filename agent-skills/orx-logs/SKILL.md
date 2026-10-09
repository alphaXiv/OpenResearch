---
name: orx-logs
description: "Locate and read persisted experiment output with `orx logs`. Invoke when inspecting run progress, debugging a run failure, or retrieving recorded configuration and measurements; use `orx-results` to interpret the findings."
---

## Reading run logs — `orx logs`

A run's terminal output is captured live while it runs and persisted afterwards.

```sh
orx logs <runId>                    # local path, byte size, and last ~500 characters
rg -n 'metric|summary' <log-path>   # search the file for relevant evidence
```

- The command prints the local log path, exact byte size, a short preview of
  the end, and a search hint. The preview is not the complete result.
- Use native file tools on the printed path to search or read the portions you
  need. Cite the lines supporting each claim.
- `<runId>` comes from `orx runs <projectId>`.

Truncated output is not evidence of absence. Search or read the reported file until
the relevant portion has been found.
