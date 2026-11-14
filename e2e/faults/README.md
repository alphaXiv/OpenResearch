# Faults: deliberately broken runs

A test bed for everything that watches a run. Each scenario launches a healthy parent, then a child under
it that goes wrong in one way, and records what orx showed while it did.

```text
payload.mjs    an invented training run: setup (4 shards), train (600 steps), eval (6 tasks), a table, ::orx:: lines
               and plain log lines; `node payload.mjs VARIANT`, one step per 100 ms. The variants are the faults.
view.xml       the view an agent would set on the baseline: better, a headline, a chart with the parent's ghost
scenarios.ts   id, the child's title, the fault in one sentence, the seconds to look at, when to stop it
run.ts         runs scenarios against the real `orx up` in a sandbox and prints the timeline
```

```sh
cargo build && (cd ui && pnpm exec vite build)    # once
node e2e/faults/run.ts --list
node e2e/faults/run.ts nan-400                    # one scenario, about 2.5 minutes
node e2e/faults/run.ts hang --keep                # leave the server up to watch it in a browser
```

The timeline prints at fixed seconds after the child starts and goes to `.shots/faults/<id>.txt`. Each sample is
the chat's line and strip, the chart captions, `orx exp status`, and the monitor's own flag, sentence, watcher
and rules. At the end: the experiment page, the log, the message that wakes the chat's agent, and
`orx exp status`. Scenarios share nothing, so several can run at once:

```sh
for s in lr-x100 nan-400 crash; do node e2e/faults/run.ts $s > /tmp/$s.out & done; wait
```

## Reading it against the plan

The server runs as the e2e suite does, `ORX_TEST_SPEEDUP=20`: a debug build divides its paced timers by 20, so
a scenario second stands for 20 seconds of a real run (100 ms a step is 2 s a step; 600 steps are 20 minutes).
A timer that does not go through `config::paced`, such as the estimator's 90 s stall threshold, runs in real
time and shows later in a compressed run.

To use a scenario from a project of your own, make its run command `node <this directory>/payload.mjs VARIANT`.

## Adding one

Add a variant to `payload.mjs` (it changes only what is broken: the same step gives the same number in every
variant, which is what makes "level with the parent" mean something), then a row in `scenarios.ts`.
