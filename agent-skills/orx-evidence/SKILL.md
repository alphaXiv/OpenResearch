---
name: orx-evidence
description: "Prepare and inspect experiment run evidence: design stdout metrics and summaries, locate persisted logs with `orx logs`, and validate run-derived claims. Use before launching a run whose output must be judged, after a run finishes, or before analyzing or reporting run results."
---

Run logs are the evidence channel. Make the run command print everything needed
to judge the result, then locate its log with `orx logs` and inspect the file.

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

## Make the run print its own evidence

Print everything needed to stdout: final metrics, a compact summary, and the key
configuration. If a run's result is not in its log, it cannot be inspected later.

- Print final metrics and a compact summary block at the end of the run, not just
  scattered during training.
- Echo the configuration the run actually used so the log identifies the variant.
- For a long run, print periodic one-line metrics so its trajectory remains
  searchable in the file.

## Validate before reporting

Never infer a result from run status or memory. Before accepting or reporting a
run-derived claim, confirm that:

- the log identifies the variant and effective configuration;
- the final metric and compact summary are present;
- the relevant trajectory is recoverable for a long run; and
- the cited file lines actually contain the supporting output.

Truncated output is not evidence of absence. Search or read the reported file until
the relevant portion has been found. Format the resulting chat response using
the evidence-and-links contract in the session playbook.
