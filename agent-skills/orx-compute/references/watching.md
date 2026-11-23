# Who watches a run

The OpenResearch server watches every run and catches up when reopened; jobs
keep running while it is closed. No waiting command or live agent session is
needed to keep runs monitored.

A run launched by orx 0.2 may still have a detached supervisor process watching
it; do not kill it, and the server takes the run over once it exits.

Use `orx exp wake <expId>` to subscribe this chat to a run launched elsewhere.
`orx exp wait` remains available for humans, never to keep an agent waiting.

## Interim results

A run can post an interim result by appending a line to `$ORX_NOTIFY_FILE`
(`>>`) when set; it arrives at most once per run per 10 minutes, with later
lines coalesced.
