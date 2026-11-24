# Changing a view while the run goes

The user may ask mid-run to chart something else or to compare differently,
and the run itself may show that the view misleads: a stage outside the plan,
a stat compared with the wrong point of the parent, a curve flattened by one
early spike (`y-scale="log"`). Change it then, on each experiment it concerns
(siblings each hold their own copy), with `orx exp view set <expId>`:

- `--file`: every open page redraws from the new view.
- `--pattern`: the run's kept log is read again with them.
- `--config`: the bar redraws, for runs that declared no plan of their own.

What the code reports is fixed for that run, but what it prints is not all
lost: a pattern over a line the run already prints (`references/patterns.md`)
reads it into measures and rows, the kept log included. Asked mid-run for a
chart of numbers the run prints but does not report, add a pattern and change
the running experiment's view at once, in the same turn; never edit the harness
while it runs or wait for the next run to show it. A measure the run never
prints at all needs a code change, and the next run.

- `references/view.md` — the elements, attributes and comparisons a new view can use, and which chart choices the user may have changed
- `references/table-charts.md` — when the chart in question plots table rows across sizes or kinds
