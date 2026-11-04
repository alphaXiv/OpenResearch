---
name: orx-logs
description: "Locate and read persisted experiment output with `orx logs`. Invoke when inspecting run progress, debugging a run failure, or retrieving recorded configuration and measurements; use `orx-results` to interpret the findings."
---

## Reading run logs — `orx logs`

A run's terminal output is kept on this computer when the run finishes. While
it runs, engine-owned output stays on its compute host; supervised runs also
stream it into a local log.

```sh
orx logs <runId>                    # local path, byte size, and last ~500 characters
rg -n 'metric|summary' <log-path>   # search the file for relevant evidence
```

- For a finished run the command prints the local log path, exact byte size, a
  short preview of the end, and a search hint. For a run still in flight it
  prints the size so far and the end of the available log. The preview is not
  the complete result.
- Use native file tools on the printed path to search or read the portions you
  need. Cite the lines supporting each claim.
- `<runId>` comes from `orx runs <projectId>`.

Truncated output is not evidence of absence. Search or read the reported file until
the relevant portion has been found.
