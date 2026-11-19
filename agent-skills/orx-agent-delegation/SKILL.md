---
name: orx-agent-delegation
description: "Delegate tasks to other models and chats with the orx MCP server's `delegate`, `wait`, `subscribe` and `answer` tools. Covers what to delegate (scouts, parallel prep, independent checks), which routes and data need the user's go, standalone briefs, worktrees, compute authorization, nesting and questions. Use before delegating or opening another chat, or when a step has parallel parts."
---

# Delegate to another chat

The orx MCP server's tools are the only way to start another chat. `models`
lists the versioned models you can name. `delegate` starts a chat on one and
returns at once; you are subscribed to it, so its answer, or a question it
asks, arrives here as a new message. Call `wait` only when you cannot continue
without the answer, and end your turn otherwise; never poll `status` in a loop.

## What to delegate

When a step has parts another chat could do while you work, delegate them and
keep the main line here:

- **Scouts** on a fast model: candidate papers (`orx-lit-review` says what to
  return), a dataset's format, a library's API.
- **Prep**: data loading, an eval harness, a figure script, in its own files or
  `worktree: "own"`.
- **Independent checks** of a claim a decision rests on, by another provider's
  model once the user approved that provider (below). Give it the
  claim, the runs and the files, not your conclusion, and name the checking
  model when you report the claim.

Keep the decision, the synthesis and what you tell the user in this chat. Tell
the user what a delegate's result changes, not that it finished.

A model in `models` is available, not approved. A delegate gets this
project's checkout, so delegate only to a model provider the user already uses
in this project's chats or approved for it. A route billed per use, such as an
API key or a metered provider, is spend even when this chat runs on it: it
needs a budget that covers it, with what runs and delegates in flight already
committed counted against it, or the user's go; skip optional delegation whose
cost you cannot bound. A route marked `(free)` adds no charge. Name the route you checked in `delegate`'s `route`. Delegation is
encouraged, never required: when no permitted route exists, do the in-scope
work yourself, and never end a turn just to ask for a delegate.

A delegate starts with nothing but its task. Write a standalone brief: the
project, experiment or commit, constraints, permitted compute, expected output,
and definition of done. Explicitly forbid compute launches when none are
authorized. To pass the user's own words or a screenshot, cite the message's
ref (`msg_3fa9c1d2`) in the task instead of retyping it.

- **Worktree.** By default a delegate works in this chat's worktree, so give
  parallel delegates separate files. `worktree: "own"` gives it a fresh
  worktree from the project's baseline; it is still your delegate, so it stays
  out of the sidebar. Commit first and name the commit in the task when it
  should start from your work. Its changes never merge here automatically.
- **Independent chats.** `notify: false` starts a chat that is not your
  delegate: your side chat, or with `worktree: "own"` a top-level chat in the
  user's sidebar. Start a top-level chat only when the user asks for one. You
  do not hear from it unless you `subscribe`, and you cannot `cancel` it. Do not open one as a substitute for
  a follow-up message; `send` the chat you already have.
- **Nesting.** A delegate may delegate only when you allow it with
  `canDelegate` or the user's setting does. Keep nesting shallow.
- **Questions.** `answer` a delegate's question when the brief or the
  direction the user approved decides it; otherwise ask the user for that
  specific decision. Approvals are the user's.
- **After a model switch or cut-off.** Check each delegate named in the chat
  with `status` before starting new work. Before relaunching one whose chat
  ended without its answer, `read` what it already did (commits, runs, files)
  and why it ended. Relaunch only work still needed, whose result or artifact
  is not already there, and whose failure has a known, fixable cause, such as
  a cut-off or restart; brief it with the work already done. Never relaunch one
  the user stopped.
- **Effort.** Pass `effort` from `models` only when the task needs more or less
  reasoning than the route's default.
- **Retries.** Give `delegate` and `send` a `clientRequestId` when you may
  retry, so a retry does not start the work twice.
