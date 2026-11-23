# Shape the tree: stacked bushes, not a flat fan or a noodle

The most common way to drive a project badly is to get the **shape** wrong.
There are two opposite failures, and the right shape sits between them:

```
FLAT FAN (wrong)            NOODLE (wrong)            STACKED BUSHES (right)
root                        root                      root
├ a ├ b ├ c ... ├ n         └ a                       └ lr-head        ┐ round 1:
                              └ b                        ├ lr 2e-5     │ a small fan of
                                └ c                      └ lr 3e-5     ┘ co-equal options
                                  └ d ...                   └ winner ── arch-head   ┐ round 2
                                                               ├ arch-A             │ descends onto
                                                               └ arch-B             ┘ round 1's winner
```

- **Flat fan** (the whole sweep hanging off the root): every result is measured
  against the *start*, so wins never accumulate and the tree never makes progress.
- **Noodle** (a long single-child chain): depth manufactured for its own sake;
  each step doesn't actually build on the one above it.
- **Stacked bushes** (correct): a *small fan within a round* (the options of one
  decision), then **descend onto that round's winner** for the next round.

**The one rule that produces this shape.** Before you make X a child of Y, name
what Y established that X builds on:

- **You can name it** ("Y is the LR winner; X keeps that LR and changes the
  architecture"): real depth. X is a **child** of Y. Descend.
- **You can't, because X and Y are co-equal options tried at the same time**
  (lr 2e-5 vs lr 3e-5): they don't build on each other. They're **siblings** in
  the same bush. Fan, don't chain.

So **width = the open options of one decision** (fan freely: a 3-way LR sweep
*should* be three siblings under a common head) and **depth = decisions already
resolved, stacked** (one level down per winner kept). A new *round* never hangs
off the root; it hangs off the previous round's winner. That keeps the tree
moving **downward** as research progresses, without stringing unrelated nodes
into a line.

Re-read the tree each round. `orx project view` lists every node
(id, title, branch; roots marked `[root]`). Check the shape: a wide row of
direct children off the root with no grandchildren means you're fanning when you
should be descending; a long chain with no branching means you're chaining
co-equal variants that should have been siblings.
