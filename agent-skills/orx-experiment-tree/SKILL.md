---
name: orx-experiment-tree
description: "Plan and drive the experiment tree: first-launch setup, fixed run contract, frozen nodes, stacked-bush tree shape, planned next rounds, branch/launch/reconcile/promote, repair limits, and notes. Use before creating or changing experiments, launching a first run, deciding what to try next, handling a completed run or any other wake, or reporting experiment progress."
---

A project is a **tree of experiment nodes**. The root (**baseline**) holds the
starting code and a **run command** — the single shell command that trains or
evaluates the node and prints its results to the run log. Every other node is a
**child** branched off a parent, inheriting its code and its run command. The two
rules this depends on — **never edit a node a run has answered** and
**the run command + env is a fixed contract** — are the cardinal rules;
everything below assumes them.

Create a node only when a planned run will establish a baseline or test a
hypothesis relevant to the project. Put a code change on that node only if it
serves that baseline or hypothesis. Do not create nodes for unrelated cleanup,
refactors, bug fixes, or dependency updates.

## Before the first launch

Follow the session playbook's Python policy. Before launching, resolve the
train/evaluation command and compute-specific requirements; ask only if the
project setup leaves these or the chosen workflow unclear. Record the durable
setup and execution recipe in the project's run command.

Make the baseline followable before its first launch: its stages, the
measures that answer the question, and the view that draws them. Load
`orx-experiment-runs` and follow it; children inherit all of it.

## Provisional until it answers — repair, don't branch

Every node exists to establish a baseline or test a hypothesis. A run that dies
on an error does **neither** — nothing was established, nothing was tested — so
there is nothing to protect: fix that node's branch in place and re-run the
same node. Successive runs on one node are how you get it working.

Once a run *does* answer the node — it produced the result the node was after,
good, bad, or `nan` — the node is **frozen**. Its branch is the code that
result came from: never edit it again, branch a child instead. That holds
however the run ended, and it is permanent — a disappointing number is a
result, not a reason to repair.

Unintended behaviour is not an answer. An OOM, a timeout, a divergence from a
bug, a missing dep — those are implementation and hardware details, and the
node is still provisional (unless the node's hypothesis *is* about memory or
runtime, in which case that outcome is exactly its result).

**Repair cap:** two runs in a row that answer nothing on one node, then ask the
user. Different errors still count; a bare relaunch or a flavor/backend switch
is a repair. If the same failure hits a second node, that is one setup problem
— ask then. (Separate from the "~3 failed or regressed runs" scientific stop.)

## Shape the tree — stacked bushes, not a flat fan or a noodle

The single most common way to drive a project badly is to get the **shape** wrong.
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

- **Flat fan** (your whole sweep hanging off the root): every result is measured
  against the *start*, so wins never accumulate and the tree never makes progress.
- **Noodle** (a long single-child chain): depth manufactured for its own sake —
  each step doesn't actually build on the one above it.
- **Stacked bushes** (correct): a *small fan within a round* (the options of one
  decision), then **descend onto that round's winner** for the next round.

**The one rule that produces this shape.** Before you make X a child of Y, name
what Y established that X builds on:

- **You can name it** ("Y is the LR winner; X keeps that LR and changes the
  architecture") → real depth. X is a **child** of Y. Descend.
- **You can't — X and Y are co-equal options you're trying at the same time**
  (lr 2e-5 vs lr 3e-5) → they don't build on each other. They're **siblings** in
  the same bush. Fan, don't chain.

So: **width = the open options of one decision** (fan freely — a 3-way LR sweep
*should* be three siblings under a common head); **depth = decisions already
resolved, stacked** (one level down per winner kept). A new *round* never hangs off
the root — it hangs off the previous round's winner. That keeps the tree moving
**downward** as research progresses, without stringing unrelated nodes into a line.

Re-read the tree each round — `orx project view <projectId>` lists every node
(id, title, branch; roots marked `[root]`) — and check the shape: a wide row of
direct children off the root with no grandchildren means you're fanning when you
should be descending; a long depth-N chain with no branching means you're chaining
co-equal variants that should have been siblings.

## The auto-research loop

To drive a project toward a goal (e.g. "best convergence for d=8"), this is the
intended flow — do **not** edit a frozen node or rewrite the run command:

1. **Read the baseline's code.** You already sit in a private Git worktree of the
   project's repository. Check out the branch and read it with your normal tools
   (see `orx-git`). See the node's run command with `orx exp status <expId>` and
   find where the knobs live (config files, hyperparameters, model definitions).
2. **Form one round's worth of hypotheses** — the co-equal options of a *single*
   decision (which LR? which schedule? which init?), each a concrete change you can
   make and measure against the others in this round. Don't mix decisions from
   different rounds into one batch — that's what produces the flat fan.
3. **Create the round as a bush, or confirm the one planned in step 6, and pick
   its parent deliberately.** All of this round's options are **siblings under
   one parent** — the title is the idea, the
   description is the concrete change you'll make on that node's branch. The user
   reads the title beside the run's progress, among its siblings: keep it short,
   the setting that varies first ("Depth 4 at 2e15 FLOPs", "LR 3e-4"). The parent is:
   - the **baseline**, only for the very first round (nothing has been won yet); or
   - the **previous round's confirmed winner**, for every round after — so this
     round's changes build *on top of* the last gain instead of resetting to the
     start. This is what walks the tree downward (see "Shape the tree" above).

   ```sh
   # Round 1 — one decision (the LR), its options fanned off the baseline:
   orx create-experiment <projectId> --parent <baseId> --title "LR 2e-5" \
     --description "Set the LR in config.yaml to 2e-5; change nothing else."
   orx create-experiment <projectId> --parent <baseId> --title "LR 3e-5" \
     --description "Set the LR in config.yaml to 3e-5; change nothing else."

   # Round 2 — LR 3e-5 won → the next decision (architecture) descends onto it:
   orx create-experiment <projectId> --parent <lr3e5WinnerId> --title "Wider MLP" \
     --description "On top of the LR-3e-5 winner, widen the MLP hidden dim 1024→2048 in model.py."
   ```
   The child inherits its parent's run command automatically — you don't set it,
   and you never give siblings different commands or env vars (cardinal rule 2).
4. **Implement each child's change on its Git branch** just before launching
   it — `orx create-experiment` prints the child's branch (`orx/<slug>`); in
   your worktree:
   ```sh
   git checkout -B orx/<child-slug> <parent-branch>   # child has no commits or runs yet
   #   …edit only the files that idea touches…
   git commit -am "cosine LR + warmup"
   ```
   `-B` starts the branch from its parent's current code, which a repair or an
   `orx exp move` may have changed since the child was planned. Use plain
   `git checkout orx/<child-slug>` once the child has commits.
   **Leave the run command alone.** Before launching, ensure the committed code
   prints its effective configuration, final metrics, and a compact summary to
   stdout. Long runs should also print periodic searchable metrics.
5. **Launch the round's ready children**: `orx exp run <childId> --backend <b>`
   (or omit `--backend` when a default target is set — see `orx-compute`). Remote
   backends can run siblings in parallel; `--backend local` shares this machine's
   CPU, RAM, and GPU.
6. **Plan the next round before this one lands, then end your turn.** Create
   the next decision's options as unrun children, title and description only:
   under the running node when one run answers this step, or under the sibling
   you expect to win when several compete. These planned children are the queue;
   the user sees them in the tree, so never keep a plan in chat or a file. Run
   results and delegates' answers arrive as messages in this chat once it is
   idle; no waiting process or live agent session is needed.
7. **Reconcile on every wake.** A run result, a delegate's answer, a flag, a
   model switch or a resumed chat all start the same way: read
   `orx project view <projectId>` and `orx runs <projectId>`, and check each
   delegate named in the chat with the MCP `status` tool. Account for every run
   still going, every delegate still working and every planned child, including
   runs that finished while you analyzed another. Relaunch a delegate whose chat
   ended without its answer. When every run is terminal and no planned child of
   this round remains, the round is complete.
8. **Analyze each finish as it lands, then iterate.** When a result message
   arrives, read its results from the file reported by `orx logs <runId>`
   (see `orx-logs`) and, when it logged to Trackio, its curves with
   `orx metrics <runId> <parentRunId>`, then use `orx-results` to interpret the outcome. To see exactly what a finished node changed, diff its
   branch against its parent's
   branch (see `orx-git`). Don't infer from status alone. Each
   completion is a decision point with four moves:
   - **Repair** — the run answered nothing: fix this node's branch and
     re-launch the same node (above).
   - **Refill** — result is mediocre or inconclusive: launch the next planned child to
     keep the round moving (step 5).
   - **Promote** — result is a clear win: this node becomes the **parent for the next
     round**. The next batch of children branch off *it*, not the baseline, so the win
     carries forward and the next ideas stack on top of it. If you planned that round
     under another sibling, `orx exp move` its unrun children onto the winner. This is
     the move that makes the tree grow deeper; skipping it is what produces a flat,
     sweep-only tree.
   - **Stop** — goal met, or the branch is exhausted.

   After each move, update the planned children: add what the result suggests and
   archive (`orx exp archive <expId> --only`) those it made pointless. Frozen nodes
   stay untouched throughout — promotion moves the *focal parent* down the tree, it
   never rewrites a node that already measured something.

After ~3 consecutive failed or regressed runs on a branch, reconsider it: back up
to an earlier winner or change the decision being tested. Stop when the goal is
met or the agreed budget is spent, and write up the tree as a descriptively named
project artifact — see the `orx-reports` skill for naming and folder guidance.
Findings live in such artifacts and node notes, never only in chat.

## Experiment description / notes — `orx exp desc`

Each experiment node carries a free-form **description** (markdown) — the same
field set by `create-experiment --description`. Use it for notes: observations,
hypotheses, or a running summary. It is a whole-document field: writing
overwrites whatever was there.

```sh
orx exp desc <expId>                          # print the description to stdout (empty → hint on stderr)
orx exp desc <expId> --set "tried lr=3e-4, diverged at step 4k"   # overwrite with a short note
cat notes.md | orx exp desc <expId> --stdin   # overwrite from stdin (long markdown)
```

- **Read** prints the text to **stdout** (pipe/redirect-friendly); when empty, a
  hint is printed to **stderr** and stdout stays empty.
- **Write** with exactly one of `--set` (inline) or `--stdin` (whole of stdin).
  Passing both is an error. Writing **replaces** the entire description — to
  append, read first, edit, and write back.
- `<expId>` comes from `orx create-experiment` output or `orx project view
  <projectId>` (the experiment id, not a run or project id).

## Restructure the tree — `orx exp move` and `orx exp archive`

Fix the tree's shape when a node or subtree ended up under the wrong parent,
instead of recreating nodes and re-running them.

```sh
orx exp move <expId> --parent <newParentId>   # re-parent the node and its subtree
orx exp move <expId> --baseline               # make it a new root
orx exp archive <expId> --only                # hide superseded nodes (also --ancestors/--descendants)
orx exp unarchive <expId> --only              # restore them
orx exp delete <expId>                        # permanently remove a leaf node, its runs and branch
```

- `move` changes only the tree; runs and results stay attached to the node.
  Its branch is not rebased and still holds the old parent's code, so diffs
  then compare against the new parent's branch (or the project base branch
  for `--baseline`). Move only nodes whose code fits the new parent.
- Don't recreate a node that has runs just to fix its place in the tree: move
  or archive it so its results keep their provenance.
- `delete` cannot be undone except for the branch (it prints the commit to
  restore it from). Use it only when the user asks to remove a node
  permanently; archive otherwise. It refuses nodes with children or active
  runs, and branches checked out in any worktree.
