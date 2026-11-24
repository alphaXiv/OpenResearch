---
name: orx-experiment-tree
description: "Plan the experiment tree and what to try next. Use before creating, editing or restructuring experiments, after a run finishes, and when reporting progress."
---

A project is a **tree of experiment nodes**. The root (**baseline**) holds the
starting code and a **run command**: the single shell command that trains or
evaluates the node and prints its results to the run log. Every other node is a
**child** branched off a parent, inheriting its code and its run command.

Create a node only when a planned run will establish a baseline or test a
hypothesis relevant to the project, and put a code change on it only if it
serves that. No nodes for unrelated cleanup, refactors, bug fixes or
dependency updates.

## Cardinal rules

1. **Never edit a node once a run has answered it.** Its branch is the code that
   result came from; branch a child instead. A run that answered nothing (a
   crash, an OOM, a missing dep) leaves the node provisional: fix it in place
   and re-run the same node.
2. **The run command and environment are a fixed contract.** Children inherit
   them; never rewrite them or give siblings different commands or env vars.
3. **Vary code, not knobs in the command.** Each idea is a change committed on
   its node's branch.
4. **Grow the tree downward.** Siblings are the co-equal options of one
   decision; the next round descends onto the previous round's winner.

## The loop

1. Read the parent's code in your worktree (`orx-git`) and its run command with
   `orx exp status <expId>`.
2. Form one round: the co-equal options of a single decision.
3. Create each option as a sibling under the baseline (first round only) or the
   previous round's winner. Short title, the setting that varies first:
   ```sh
   orx create-experiment --parent <parentId> --title "LR 3e-5" \
     --description "Set the LR in config.yaml to 3e-5; change nothing else."
   ```
4. Commit the change on the printed branch (`orx/<slug>`); leave the run command
   alone.
5. Launch with `orx exp run <childId>` (`orx-compute`), then end your turn.
   Results arrive as messages in this chat.
6. On each message, reconcile every newly terminal run in `orx runs`,
   read its result from the file `orx logs <runId>` reports (`orx-results`
   interprets it), then **repair**, **refill**, **promote** the winner, or **stop**.

Stop when the goal is met, or after ~3 consecutive failed or regressed runs.
When you stop, write up the tree as a descriptively named project artifact (see
`orx-reports` for naming and folders).

Close any turn that ran or changed experiments with a short experiment summary:
one line per relevant node with what it tested, its status, and the headline
result. Tag each result as the playbook's Replies section says. Plain
questions and turns that launch or change no experiments need no summary.

## Read when

If a reference beside this skill can't be read, `orx skill experiment-tree/<name>`
prints it.

- references/first-launch.md — before the baseline's first run
- references/loop.md — driving a project toward a goal: each loop step in full, with examples and the four moves after a result
- references/repair.md — a run failed, or ended without answering its node
- references/tree-shape.md — choosing a new node's parent, or checking the tree's shape at the start of a round
- references/notes.md — reading or writing an experiment's notes with `orx exp desc`
- references/restructure.md — before moving, archiving, unarchiving or deleting an experiment
