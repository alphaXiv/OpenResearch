---
name: orx-create
description: "Initialize a project with `orx up` and add experiment nodes with `orx create-experiment`. Use when starting a project or experiment, when the tree is empty, or when choosing a baseline, parent, or run command."
---

## Start a project

Run `orx up`, then use the dashboard to import an existing Git repository or
create a new project. Optional GitHub publication is for collaboration and is
not required for compute.

For an existing GitHub repository, clone it normally before importing it:

```sh
git clone https://github.com/<owner>/<repo>
orx up
```

Prefer an existing implementation over starting from a blank repository. For a
paper, use `orx paper <id>` and the `orx-lit-review` retrieval workflow to find the authors' or best
community implementation, clone it, then import that checkout in `orx up`.

Set the project's run command once before launching experiments:

```sh
orx project edit <localProjectId> --run-command '<command>'
```

## Add an experiment node

`orx create-experiment` accepts project and experiment ids:

```sh
# First node in an empty project: the baseline root.
orx create-experiment <localProjectId> --title "Baseline"

# Child node branched from an existing experiment.
orx create-experiment <localProjectId> --title "Larger batch" \
  --parent <localExperimentId> \
  --description "Increase batch size and compare throughput and loss."

# Explicit additional root when the project already has one.
orx create-experiment <localProjectId> --title "Alternative baseline" --baseline
```

- The first parentless node is the baseline. Later parentless nodes attach to
  the oldest root unless `--baseline` explicitly requests another root.
- A child branches from its parent's Git branch and inherits its run
  command. Vary code or configuration on the child; keep the command fixed.
- `--description` should state the concrete change and measurement the node
  owes.
- Commit the node's branch before launching. Runs use an immutable archive of
  that local commit.
