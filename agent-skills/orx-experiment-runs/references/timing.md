# Timing a run and sizing it to a duration

The bar times each stage by the experiment's own last run of it, else the
parent's, scaled to this run's total; a run that counted the stage in another
unit, or where only one of the two declared a total, times nothing, and the
stage stays untimed. A stage the run reaches outside the plan shows as
unplanned, and a planned stage it passes over, or finishes without, shows as
skipped: both mean the plan is wrong, so fix it. A cancelled or failed run's
later stages are only unreached.

## When the user names a duration or a budget

Size the run with the estimate `orx-compute` has you run before every launch.
If the estimate falls below the user's requested window, add steps or data or
run a short calibration before launching, and tell the user which you did.
Resize the workload in the child's code or configuration and update its stage
plan. Re-estimate until the total falls inside the user's range. If the
estimate says "at least" or "unknown," report that and calibrate the untimed
stages before promising a duration or cost; an unknown total cannot be
re-estimated into the range.
