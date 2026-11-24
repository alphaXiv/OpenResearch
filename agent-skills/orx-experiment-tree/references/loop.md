# The auto-research loop

To drive a project toward a goal (e.g. "best convergence for d=8"), this is the
intended flow. Never edit a frozen node or rewrite the run command.

1. **Read the baseline's code.** You already sit in a private Git worktree of the
   project's repository. Check out the branch and read it with your normal tools
   (see `orx-git`). See the node's run command with `orx exp status <expId>` and
   find where the knobs live (config files, hyperparameters, model definitions).
2. **Form one round's worth of hypotheses**: the co-equal options of a *single*
   decision (which LR? which schedule? which init?), each a concrete change you
   can make and measure against the others in this round. Don't mix decisions
   from different rounds into one batch; that produces a flat fan.
3. **Create the round as a bush, and pick its parent deliberately.** All of this
   round's options are **siblings under one parent**. The title is the idea, the
   description is the concrete change you'll make on that node's branch. The
   user reads the title beside the run's progress, among its siblings: keep it
   short, the setting that varies first ("Depth 4 at 2e15 FLOPs", "LR 3e-4").
   The parent is:
   - the **baseline**, only for the very first round (nothing has been won yet); or
   - the **previous round's confirmed winner**, for every round after, so this
     round's changes build *on top of* the last gain instead of resetting to the
     start. This is what walks the tree downward (see references/tree-shape.md).

   ```sh
   # Round 1: one decision (the LR), its options fanned off the baseline.
   orx create-experiment --parent <baseId> --title "LR 2e-5" \
     --description "Set the LR in config.yaml to 2e-5; change nothing else."
   orx create-experiment --parent <baseId> --title "LR 3e-5" \
     --description "Set the LR in config.yaml to 3e-5; change nothing else."

   # Round 2: LR 3e-5 won, so the next decision (architecture) descends onto it.
   orx create-experiment --parent <lr3e5WinnerId> --title "Wider MLP" \
     --description "On top of the LR-3e-5 winner, widen the MLP hidden dim 1024→2048 in model.py."
   ```
   The child inherits its parent's run command automatically. You don't set it,
   and you never give siblings different commands or env vars.
4. **Implement each child's change on its Git branch.** `orx create-experiment`
   prints the child's branch (`orx/<slug>`); in your worktree:
   ```sh
   git checkout orx/<child-slug>
   #   …edit only the files that idea touches…
   git commit -am "cosine LR + warmup"
   ```
   **Leave the run command alone.** Before launching, ensure the committed code
   prints its effective configuration, final metrics, and a compact summary to
   stdout. Long runs should also print periodic searchable metrics.
5. **Launch the round's ready children**: `orx exp run <childId> --backend <b>`
   (or omit `--backend` when a default target is set; see `orx-compute`). Remote
   backends can run siblings in parallel; `--backend local` shares this
   machine's CPU, RAM, and GPU.
6. **Launch, then end your turn.** Relevant run results arrive as messages in
   this chat once it is idle. No waiting process or live agent session is
   needed. On each message, read `orx runs` and reconcile every
   newly terminal run, including those that finished while you were analyzing
   another result. Analyze each result and refill its slot as appropriate;
   other runs can keep going. End your turn again after launching ready
   children. When every run is terminal and no children remain to launch, the
   round is complete.
7. **Analyze each finish as it lands, then iterate.** When a result message
   arrives, read its results from the file reported by `orx logs <runId>` and, when it logged to Trackio, its curves with
   `orx metrics <runId> <parentRunId>`, then use `orx-results` to interpret the
   outcome. To see exactly what a finished node changed, diff its branch
   against its parent's branch (see `orx-git`). Don't infer from status alone.
   Each completion is a decision point with four moves:
   - **Repair**: the run answered nothing. Fix this node's branch and re-launch
     the same node (see references/repair.md).
   - **Refill**: the result is mediocre or inconclusive. Launch the next queued
     child to keep the round moving (step 5).
   - **Promote**: the result is a clear win. This node becomes the **parent for
     the next round**. The next batch of children branch off *it*, not the
     baseline, so the win carries forward and the next ideas stack on top of
     it. This is the move that makes the tree grow deeper; skipping it is what
     produces a flat, sweep-only tree.
   - **Stop**: goal met, or the branch is exhausted.

   Frozen nodes stay untouched throughout: promotion moves the *focal parent*
   down the tree, it never rewrites a node that already measured something.
