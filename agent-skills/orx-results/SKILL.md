---
name: orx-results
description: "Interpret and summarize measured experiment results. Invoke before reporting measured results, including summarizing a completed run, comparing runs, evaluating an outcome, or recommending a next experiment from run evidence; retrieve missing output with `orx-logs`."
---

## Validate before reporting

Never infer a result from run status or memory. Before accepting or reporting a
run-derived claim, confirm that:

- the log identifies the variant and effective configuration;
- the final metric and compact summary are present;
- the relevant trajectory is recoverable for a long run; and
- the cited file lines actually contain the supporting output.

Load `orx-logs` when supporting output still needs retrieving. Cite the verified
measurements using the session playbook’s evidence-and-links contract.

## Summarize an experiment

Choose one primary representation for each measured comparison: a graph, a
table, or prose. Explain the conclusion and limitations in brief prose.

Choose prose and selective visuals that make the result easiest to understand.
If you choose to use multiple representations or visuals, make sure they do not
contain duplicate data or show the same thing two different ways. They should
each offer a distinct way of interpreting the experiment’s results — one visual
will suffice for most experiments.
For example, do not show both a table and graph of the same data — just show the graph.

An interactive graph and linked source data already provide exact-value lookup;
a table of those values is not a useful fallback. Subsetting, condensing, or
deriving values from the same measurements does not make a second representation
complementary. Put useful headline values or changes in the prose interpretation
instead. Before sending, remove any table or visual that repeats a comparison
already represented.

Briefly explain what changed, what ran, and what the evidence shows. When the
measured results suit a supported `orx-figures` template and a graph makes the
result easier to understand, prefer an interactive figure inline alongside
that summary. Reuse an existing figure where possible and link its source data.

This is a rule of thumb, not a required output. Skip the visualization when
there is no suitable structured data, no appropriate template, or no useful
visual comparison. Do not invent data, additional runs, or graph views to
satisfy the preference; a concise text summary can be the complete result.

Default to one graph. Offer a dropdown only when multiple views of the same
data answer distinct, useful questions. Multiple series alone do not require
a dropdown, and one useful view does not need a selector.
