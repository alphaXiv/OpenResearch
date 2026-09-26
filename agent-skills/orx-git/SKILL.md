---
name: orx-git
description: "Read, edit, commit, and diff experiment code; coordinate shared worktrees and preserve frozen branch history. Use before branch work, when starting work alongside other sessions, comparing nodes, preparing a run, repairing a provisional node, or diagnosing stale code."
---

Git records every experiment. GitHub publication may be enabled for collaborator
visibility, but it is never part of compute transport. Follow the project
playbook's publication status; do not push merely to launch compute and do not
fetch from or publish back to the paper's upstream repository.

Other chat sessions may work in sibling worktrees of the same clone. Before
starting, inspect `git branch -a`, `orx runs <projectId>`, and relevant `orx exp
desc <expId>` notes so you do not duplicate their work. Branches and remotes are
shared even though worktrees are separate. Keep experiment notes current as you
learn so sibling sessions can orient from them.

One branch has one worktree owner. If checkout says a branch is already checked
out, keep working only when the named path is your worktree; otherwise leave that
branch to its owning session. Session worktrees start detached on the baseline,
so check out a node's branch before editing that experiment.

Each experiment node has a local `orx/<slug>` branch. `orx
create-experiment` creates it from its parent. Work in the session worktree,
check out the printed branch, make only that experiment's change, and commit it:

```sh
git checkout orx/<slug>
git status --short
git add <changed files>
git commit -m "describe the experiment change"
```

Code that does not serve a node's baseline or hypothesis belongs on a new
Git branch outside `orx/`.

The runner builds an immutable source archive from the recorded commit, so
committed work is sufficient on every backend. Uncommitted files are never
included in a run. Before launching, confirm `git status --short` is empty and
inspect the recorded commit with `git show --stat --oneline HEAD`.

To compare a child with its parent, use local refs only:

```sh
git diff <parent-branch>...orx/<child-slug>
git log --oneline <parent-branch>..orx/<child-slug>
```

Never rebase experiment history; it records the exact code that ran. Commit
or merge into an experiment branch only while its node is provisional and
has no run in flight. Merge other work only if that node needs it.
