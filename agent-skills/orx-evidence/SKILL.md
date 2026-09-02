---
name: orx-evidence
description: "Prepare and inspect experiment run evidence: design stdout metrics and summaries, read persisted results with `orx logs`, and validate run-derived claims. Use before launching a run whose output must be judged, after a run finishes, or before analyzing or reporting run results."
---

Run logs are the evidence channel. Make the run command print everything needed
to judge the result, then read it back with `orx logs`.

## Reading run logs — `orx logs`

A run's terminal output is captured live while it runs and persisted afterwards.

```sh
orx logs <runId>                    # tail (the end — usually what you want)
orx logs <runId> --head             # read from the start instead
orx logs <runId> --bytes 200000     # raise the byte cap (default 64 KB, max 1 MB)
orx logs <runId> --range 4096:8192  # exact byte window [start, end)
```

- The log goes to **stdout**; a `[source] bytes a–b of N` status line goes to
  **stderr**, noting if content was truncated above or below.
- `<runId>` comes from `orx runs <projectId>`.

## Make the run print its own evidence

Print everything needed to stdout: final metrics, a compact summary, and the key
configuration. If a run's result is not in its log, it cannot be inspected later.

- Print final metrics and a compact summary block at the end of the run, not just
  scattered during training.
- Echo the configuration the run actually used so the log identifies the variant.
- For a long run, print periodic one-line metrics so its trajectory remains
  visible through byte-range reads.

## Validate before reporting

Never infer a result from run status or memory. Before accepting or reporting a
run-derived claim, confirm that:

- the log identifies the variant and effective configuration;
- the final metric and compact summary are present;
- the relevant trajectory is recoverable for a long run; and
- the returned byte window actually contains the supporting output.

Truncated output is not evidence of absence. Use `--head`, `--bytes`, or
`--range` until the relevant portion has been read. Format the resulting chat
response using the evidence-and-links contract in the session playbook.
