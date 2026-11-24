# Restructure the tree: `orx exp move` and `orx exp archive`

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
