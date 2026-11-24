---
name: orx-results
description: "Read run logs and report measured results. Use before summarizing or comparing runs, debugging a failed run, or proposing a next experiment from evidence."
---

## Find the evidence

`orx exp status <expId> --run <runId>` lists the measures a run reported and how
each compares with the parent's run; it is the quick read while a run is live.
The log is the evidence:

```sh
orx runs                            # run ids
orx logs <runId>                    # where the log is kept, its size, and its end
rg -n 'metric|summary' <log-path>   # search each file it printed
```

For a finished run, `orx logs` prints the log's local file, or several kept
ranges when the middle of a long log isn't on this computer, then the end of
it. The preview is not the result, and truncated output is not evidence of
absence: search or read each printed file with native file tools until the
relevant lines are found. A live run's output stays on its compute host: `orx logs`
prints its size so far and the end of what is available, and `orx exp status`
gives its stage and measures.

## Validate before reporting

Never infer a result from run status or memory. Before accepting or reporting a
run-derived claim, confirm that:

- the log identifies the variant and effective configuration;
- the final metric and compact summary are present;
- the relevant trajectory is recoverable for a long run; and
- the cited file lines actually contain the supporting output.

Confirm a number from `orx exp status` in the log before citing it, and cite
each claim with the playbook's run and file tags.

## Summarize an experiment

Briefly explain what changed, what ran, and what the evidence shows, with the
conclusion and limitations in prose. Give each measured comparison one
representation: a graph, a table, or prose. A second one must offer a distinct
reading, not the same data again: never a table and a graph of the same
measurements, and a subset, condensation or derivation of them doesn't count as
distinct. An interactive graph with linked source data already gives exact
values, so put headline numbers in the prose instead of a table.

When the results suit an `orx-figures` template and a graph makes them easier
to understand, prefer one interactive figure inline, reusing an existing one
and linking its source data. Skip it when there is no structured data, no
suitable template or no useful comparison; never invent data or runs to fill
one, and a concise text summary can be the whole result. Add a dropdown only
when several views of the same data answer distinct questions.
