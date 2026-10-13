---
name: orx-agent-delegation
description: "Create independent project chats with `orx agent spawn`: standalone opening messages, project selection, worktrees, compute authorization, and cross-harness review chats on other vendors' agents. Use before creating another chat or when cross-harness review is on."
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
orx agent spawn "<message>" --harness <harness> [--model <model>]
```

The project defaults to this OpenResearch chat's project; pass `--project` when
that context is unavailable. The command requires the running local server.
`--harness` runs the chat on another agent with that agent's default settings
and strongest model unless `--model` names one; use it only for cross-harness
review below. `--no-wake` is no longer supported.

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

## Cross-harness review

When the session playbook says cross-harness review is on, the user wants the
research not to rest on one vendor's agent. Each agent family has its own
habits in what it proposes, how generously it reads results, and how it
writes, so have a chat on a different vendor's harness check the steps where
those habits can bias conclusions:

```sh
orx agent harnesses                     # installed harnesses and their model vendors
orx agent spawn --harness codex --title "Independent check: <claim>" --stdin
```

- **Ideas**: before committing to the next round of experiments, ask a chat on
  another vendor to propose its own candidates from the same evidence, then
  pick from the union and say where each idea came from.
- **Results**: before a claim enters a report or paper, have a chat on another
  vendor re-derive it from `orx logs` and the run outputs without seeing your
  interpretation.
- **Writing**: have a chat on another vendor review a paper or report draft
  for overclaiming, missing baselines, and statistical errors.

Rules:

- Use only harnesses the user left on for review: the `REVIEW` column of
  `orx agent harnesses` and the playbook's list. Prefer one whose vendor differs
  from this session's; OpenCode and Cursor count by the model they run. If no
  allowed harness from another vendor is ready, say so once and continue.
- Give the review chat the evidence and the question, not your conclusion.
  Forbid experiment launches unless the user approved them.
- The review chat's verdict does not come back here. Tell the user which chat
  is checking what, and keep the claim provisional until its verdict is known
  (the user relays it, or you read it in that chat's transcript if you can).
  Record agreement, or state the disagreement.
- Keep the attribution the spawn command printed (harness and model) in
  experiment notes, reports, and the paper's methods or acknowledgements, for
  example "Result independently re-derived by a Codex (OpenAI) agent."
- Do not split the experiment loop itself across harnesses.
