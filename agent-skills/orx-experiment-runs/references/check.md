# The post-launch check, in detail

`orx exp view check <runId>` names missing reports, unmatched patterns and
disagreements with the plan. With patterns, given or saved, it folds the real
log; without, it reads what collection last saved, which can trail the log for
a run's first seconds.

`orx exp view show <runId>` describes the collected stage and plan, watcher and
flag, time left, charts and parent ghosts, stats, and names or patterns not
reported yet. It reads the card's stored data, not the user's screen.

OpenResearch also checks reporting as the run starts and sends at most one
warning about missing patterns, headline or chart data, or stage totals. When
it arrives, inspect `orx exp view show RUN`, `orx exp view check RUN` and
`orx logs RUN` before acting.

## A missing report that needs a code change

The view, plan and patterns change with `orx exp view set` while the run goes;
what the code reports does not. If the missing report requires a code change,
weigh the work already done before relaunching. In Agentic mode you may cancel,
fix and commit the code, then relaunch. In Copilot mode, ask the user before
cancelling healthy compute or relaunching. Report actual progress through long
validation stages on later launches so the run shows that work is advancing.
