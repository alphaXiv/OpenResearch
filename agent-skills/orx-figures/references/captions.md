# Write the caption with the figure

Every figure-type reference tells you to "state it in the caption". This is
what that means, and the caption is written *with* the figure, not after the
draft.

Captions in current papers run a median of about 28 words, and half are three
sentences or more. The dominant shape is a **short bold phrase naming the
claim, then the detail a reader needs to trust it**:

```latex
\caption{\textbf{LPO reaches the reward plateau in a third of the compute.}
Held-out reward against training compute for LPO and two baselines; mean of 5
seeds, bands are 95\% intervals. The dotted line is the pretrained model.}
```

- **Lead with the finding, not the setup.** "Training curves for LPO and
  baselines" names the axes, which the axes already do. The caption's first
  clause should be the thing you want remembered.
- **The figure must stand alone.** A reader who skips your Section 4 should
  still get the claim right. That is the whole job.
- **Put the method facts here**, because nowhere else in the figure has room:
  seed count, what the band or bar means, any smoothing, any normalization,
  which points were fitted and which excluded, and whether a frontier line is
  measured or a guide.
- **Say what is not shown** when it matters: a single seed, a truncated axis,
  a run cut short.
- **Do not restate the axis labels** in prose, and do not repeat the caption as
  an axes title. The title is the caption's job.

A one-panel figure earns roughly 25-40 words; a multi-panel figure earns more,
and each panel needs its own clause.
