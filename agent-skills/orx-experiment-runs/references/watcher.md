# What the watcher is told, and its flags

A cheap model looks at each run while it goes and writes the sentence on its
line. It sees a digest: the run's numbers, its newest log lines and a brief
you write; never the chat, the repository or the whole log. Write the brief
beside the plan in the config, in plain sentences, by label; children inherit
it:

```json
{"brief": {
  "question": "Does depth 8 beat depth 4 on val bpb at the same token budget?",
  "healthy": "Loss falls fast in the first 200 steps, then slowly; val bpb below the parent's by step 2000.",
  "noise": "A tokens/s dip at every eval is the eval itself. Warnings from torch.compile at start are expected.",
  "worth_a_flag": "Any line that says depth=4: this run must use depth 8.",
  "from_parent": "The parent's val bpb flattened at 1.21 after step 4000.",
  "decided": "Warm-up is 200 steps; a loss spike before step 200 is not a problem."
 },
 "watch": {"minGapS": 120, "maxGapS": 1800}}
```

`worth_a_flag` is where you name what you would want to hear about; the more
specific, the fewer false flags. A look comes when something changes (a
number leaves its trend, a new kind of warning, a stage starts or ends) and,
while output keeps coming, at least every `maxGapS` seconds. `minGapS` keeps
the looks changes bring apart; a look that ends a silence can come sooner.
Leave `watch` out for the defaults (2 and 30 minutes); widen the gaps for a run
of days, narrow `maxGapS` for one that may go wrong fast.

## When a flag wakes you

A flag is the watcher, a rule that asks, or orx's own checks (a value that is
not a number, a measure half again worse than the parent's at the same step, no
progress, a new kind of error) saying a run may be going wrong. It never stops
the run. It wakes you with the lines it saw and where they are:
it saw a digest and none of the chat, so read the run yourself
(`orx exp status <expId> --run <runId>`, `orx logs <runId>`) and confirm before
acting. If you own the choice, settle it before ending your turn, with a reason
the user reads:

```sh
orx exp flag <runId> --flag <flagId> --keep --reason 'The spike is the eval, as the brief says.'
orx exp flag <runId> --flag <flagId> --stop --reason 'Loss is NaN since step 410; lowering the learning rate.'
orx exp flag <runId> --flag <flagId> --ask --reason 'It may be fine; the user knows the dataset.'
```

`--keep` settles this flag. Later looks read what you kept and why. Say what you
verified and what evidence would change your mind. A later look can raise a new
flag. `--stop` stops it as `orx exp cancel` does. `--ask` leaves the choice to
the user, who sees Stop run, Keep going and Ask in the chat.

## Flags that keep coming

For recurring model flags, read `orx exp view get EXP --json`. Write the revised
`brief` object to a JSON file as above, preserving its other labels. Use `noise`
for expected behavior and `worth_a_flag` for evidence that needs investigation.
To space out change-triggered looks, increase `watch.minGapS` and keep the
desired `watch.maxGapS`. Save with `orx exp view set EXP --config PATH`. The
changes apply to the next look built for this run.

For a built-in stall during healthy validation, have the code report a
validation phase and advancing steps on the next launch. For a flag from a
rule, find the named rule in the experiment's config and revise it there
(`references/rules.md`). Rule edits apply to later launches.
