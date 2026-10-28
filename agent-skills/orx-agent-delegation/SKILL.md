---
name: orx-agent-delegation
description: "Create independent project chats with `orx agent spawn` using the user's New Chat settings. Covers standalone opening messages, project selection, worktrees, and compute authorization. Use before creating another chat."
---

# Create an independent chat

`orx agent spawn` creates an ordinary chat and submits its opening message,
just like New Chat in OpenResearch. It uses the user's saved agent, model,
reasoning, speed, permissions, and autonomy settings. It does not inherit this
conversation or this chat's settings.

```sh
orx agent spawn "<standalone opening message>"
orx agent spawn --title "<chat title>" --stdin
orx agent spawn "<message>" --project <project-id>
```

The project defaults to this OpenResearch chat's project; pass `--project` when
that context is unavailable. The command requires the running local server.
`--harness`, `--model`, and `--no-wake` are no longer supported.

The created chat is independent and appears in the sidebar without switching
the user's view. It has its own transcript and normal session worktree. Its
result will not be sent back here, and stopping this chat does not stop it.
Use the harness's native subagents for subordinate work that needs to report
back. Do not create a new chat as a substitute for a follow-up message.

Include the relevant project, experiment or commit, constraints, permitted
compute, expected output, and definition of done in the opening message. Use
`--stdin` for longer messages. Explicitly forbid compute launches when none
are authorized. Changes stay in the new chat's worktree and never merge here
automatically. Do not assign a branch already checked out by another session
or edit a frozen experiment node.
