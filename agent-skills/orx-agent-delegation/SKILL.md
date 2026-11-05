---
name: orx-agent-delegation
description: "Delegate tasks to other models and chats with the orx MCP server's `delegate`, `wait`, `subscribe` and `answer` tools. Covers standalone briefs, worktrees, compute authorization, nesting and questions. Use before delegating or opening another chat."
---

# Delegate to another chat

The orx MCP server's tools are the only way to start another chat. `models`
lists the versioned models you can name. `delegate` starts a chat on one and
returns at once; you are subscribed to it, so its answer, or a question it
asks, arrives here as a new message. Call `wait` only when you cannot continue
without the answer, and end your turn otherwise.

A delegate starts with nothing but its task. Write a standalone brief: the
project, experiment or commit, constraints, permitted compute, expected output,
and definition of done. Explicitly forbid compute launches when none are
authorized. To pass the user's own words or a screenshot, cite the message's
ref (`msg_3fa9c1d2`) in the task instead of retyping it.

- **Worktree.** By default a delegate works in this chat's worktree, so give
  parallel delegates separate files. `worktree: "own"` gives it a fresh
  worktree from the project's baseline that appears in the sidebar: commit
  first and name the commit in the task when it should start from your work.
  Its changes never merge here automatically.
- **Independent chats.** `notify: false` starts a chat that is not your
  delegate: your side chat, or with `worktree: "own"` a chat of its own, for
  work that should not report back. You do not hear from it unless you
  `subscribe`, and you cannot `cancel` it. Do not open one as a substitute for
  a follow-up message; `send` the chat you already have.
- **Nesting.** A delegate may delegate only when you allow it with
  `canDelegate` or the user's setting does. Keep nesting shallow.
- **Questions.** `answer` a delegate's question when the brief already
  decides it; otherwise ask the user. Approvals are the user's.
- **Effort.** Pass `effort` from `models` only when the task needs more or less
  reasoning than the route's default.
- **Retries.** Give `delegate` and `send` a `clientRequestId` when you may
  retry, so a retry does not start the work twice.
