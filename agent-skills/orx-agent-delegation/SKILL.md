---
name: orx-agent-delegation
description: "Delegate tasks to other models and chats with the orx MCP server's `delegate`, `wait`, `subscribe` and `answer` tools. Covers what to delegate (scouts, parallel prep, independent checks), standalone briefs, worktrees, compute authorization, nesting and questions. Use before delegating or opening another chat, and when a step has parts that could run in parallel."
---

# Delegate to another chat

The orx MCP server's tools are the only way to start another chat. `models`
lists the versioned models you can name. `delegate` starts a chat on one and
returns at once; you are subscribed to it, so its answer, or a question it
asks, arrives here as a new message. Call `wait` only when you cannot continue
without the answer, and end your turn otherwise; never poll `status` in a loop.

## What to delegate

Keep delegates on a step's parallel parts at every autonomy level, and more of
them in Full auto-research, so the main line never waits on work another chat
could do:

- **Scouts** on a cheap, fast model from `models`: candidate papers
  (`orx-lit-review` says what to return), a dataset's format, a library's API.
- **Prep**: data loading, an eval harness, a figure script, in its own files or
  `worktree: "own"`.
- **Independent checks** of a claim a decision rests on, by a model from a
  different provider than yours. Give it the claim, the runs and the files, not
  your conclusion, and name the checking model when you report the claim.

Keep the decision, the synthesis and what you tell the user in this chat. Tell
the user what a delegate's result changes, not that it finished.

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
  with `status` before starting new work, and relaunch one whose chat ended
  without its answer.
- **Effort.** Pass `effort` from `models` only when the task needs more or less
  reasoning than the route's default.
- **Retries.** Give `delegate` and `send` a `clientRequestId` when you may
  retry, so a retry does not start the work twice.
