---
name: orx-project-admin
description: "Share, publish, import or rename a project, and look up ids. Use when the user asks to share, publish or import a project, or you need a project, experiment, run or org id."
---

## Ids

Project commands act on this chat's project when you leave out its id;
experiment commands take an experiment id and run commands a run id. Don't mix
them.

```sh
orx projects [--json]                 # project ids
orx project view [projectId] [--all]  # details and the experiment tree, with experiment ids
orx runs [projectId] [--experiment <expId>]  # run ids, newest first
orx orgs [--json]                     # org ids for OpenResearch compute (needs login)
```

## Rename or set the run command

`orx project edit [projectId] [--name "<name>"] [--run-command "<cmd>"]`. New
experiments inherit the run command; `--run-command ''` clears it.

## Share, publish and import

Only when the user asks. `orx project share [projectId]` saves the project's
code, experiment tree, finished runs, logs, artifacts and every chat as a
`.orx` file for a collaborator (`--out <file>`; narrow chats with
`--chat <chatId>`, repeatable, or drop them with `--no-chats`; `--no-logs`
and `--no-artifacts` leave those out). Add `--publish` only when the user asks for a
link: it uploads the file to openresearch.sh, which anyone with the link can
open, and publishing again updates the same link.
`orx project unpublish [projectId]` deletes the link and its file.

`orx project import <file.orx | link> [--path <dir>]` opens a shared file or an
`openresearch.sh/p/...` link as a new local project with its tree, runs and
chosen chats.

## Sign-in

Local project, experiment and run commands need no sign-in. OpenResearch
compute, instances, publishing and account commands need `orx login`
(`--device` prints a link and code to approve on another device, the default
over SSH); `orx logout` removes the stored token. When a command reports
`Not logged in`, ask the user to run `orx login`.
