# Estimator reference

`prototype.json` keeps moments from the five run-monitor v2 `sim.js`
scenarios (2026-11-11), plus stalled snapshots. Inputs use milliseconds and a
fixed epoch offset; original JavaScript ETA answers use seconds. Each phase
occurrence owns one `[at, done]` point array; each moment occupies one line.

`original` preserves the prototype's ETA answers. `expected` records the
approved departures, with each changed moment's `reason`: observed progress
replaces the current-stage prior, an active stage without a rate waits for
measurements, the first full slice is warm-up, MAD has a 1% floor, and future
phase priors preserve their wider bounds. An observed zero is a report, never
an origin inserted by the estimator. `current_started` distinguishes a future
stage from a started stage whose clock is unknown.

`details.current` is a millisecond `[lo, mid, hi]`, `pending`, or null;
`past` holds current and whole-run overruns; `fills` follows shared `segments`.
`completed` counts observed full slices, including warm-up; `fraction` records
observed progress. `cost` holds observed USD, projected USD, projected bounds,
USD at limit, GPU-hours and uncertainty. Tests also check anchors, weights,
stalls, spend and diagnostic reasons. Unchanged answers remain checked against
`original`; numeric comparisons allow max(0.00001 ms, 1e-9 relative).

Interface tests cover off-boundary resumes, partial slices, zero reports,
buffered progress and clock skew. Cadence excludes the first report-to-report
interval; the second report starts its window. Equal timestamps collapse only
for slice fitting. A phase start is separate from its observations and is
never inserted into them. No test reads prototype source.
