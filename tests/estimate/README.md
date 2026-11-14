# Estimator reference

`prototype.json` contains 186 moments: 180 from the five run-monitor v2
`sim.js` scenarios (2026-11-11), plus six snapshots stalled mid-phase.
Inputs use milliseconds and a fixed epoch offset; original JavaScript ETA
answers use seconds. Each phase occurrence owns one `[at, done]` point array;
each moment occupies one line.

The initial zero is an observed report, as emitted by `orx_run.py` progress
creation and tqdm at start. It belongs in `points` and supplies the first
observed full slice, which is excluded as warm-up. `phase_started_at` remains
separate: it describes the occurrence start and is never inserted into reports
by the estimator. `started_at` remains the payload start for GPU hours;
machine billing retains its activation time. The earlier fixture revision
incorrectly treated these observed zeros as synthetic and removed them.

`original` preserves the prototype's own ETA answers. `expected` matches it
exactly in 139 moments: 100 numeric answers and 39 with no ETA. The remaining
47 differ only for earlier plan repairs, in disjoint groups:

- 3 exclude the sole warm-up slice;
- 6 enforce the 1% MAD floor;
- 27 preserve wider future-phase prior bounds;
- 1 combines sole warm-up exclusion with wider bounds;
- 10 combine the MAD floor with wider bounds.

The six stalled snapshots are included in those counts: three match the
prototype ETA and three have the wider-prior repair. Their observed reports
stay fixed while caller time advances. No moment needs an R3 exception with
the zero report restored. Prior bounds 0.75/1.3 around scalar prototype priors
were supplied by the original generator, not measured by the prototype.
Unflagged answers are checked against `original` exactly. Numeric comparisons
use max(0.00001 ms, 1e-9 relative).

The no-zero regime stays in interface tests: off-boundary resumes, first
observation inside a slice, and the reviewer's adapter examples. The paired
zero-report test checks the same run both ways: the observed full-slice counts
differ by exactly one, changing early prior blending. Once both recent windows
have moved past the initial slices, their rates and midpoint predictions agree.
Range widths still retain the specified 0.35/(completed+1) evidence term; the
test checks that this is their only remaining difference. Estimator production
arithmetic is unchanged.

`details.current` is a millisecond `[lo, mid, hi]`, `pending`, or null;
`past` holds current and whole-run overruns; `fills` follows shared `segments`.
`completed` counts observed full slices, including the excluded warm-up;
`fraction` records observed progress. `cost` holds observed USD, projected USD,
projected bounds, USD at limit, GPU-hours and uncertainty. Tests also assert
anchors, weights, stalls, observed spend and retained diagnostic reasons.
No committed test reads prototype source. Scratch tooling is removed after
verification.

Cadence excludes the first report-to-report interval for every occurrence,
including resumed counters. The second report starts the cadence window;
only later positive gaps are samples. Raw reports define that boundary even
when the first two share a timestamp; equal timestamps collapse only for
slice fitting. Before cadence exists, silence uses the phase prior or stays
unknown. This changes only `oe:1`'s expected stall from 0 to null; all ETA
answers, reasons and counts above remain unchanged. Fractional first-boundary
matching uses a relative floating-point tolerance, with a tiny-total guard test.
