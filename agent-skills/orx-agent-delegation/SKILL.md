---
name: orx-agent-delegation
description: "Create independent project chats with `orx agent spawn`: standalone opening messages, project selection, worktrees, compute authorization, quota-aware harness, model, and compute choice (`orx agent quota`, `orx agent route`), and cross-harness review chats on other vendors' agents. Use before creating another chat or when cross-harness review is on; keep ordinary literature retrieval local; deep-research has its own contract."
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

For ordinary `orx-lit-review`, keep the retrieval loop with the main agent.
For the `orx-deep-research` report workflow, independent subtopics may be
delegated under its bounded research and synthesis contract.

## Quota-aware delegation

Claude Code, Codex, and Antigravity can each run on several accounts, and each
account has its own quota. Antigravity meters its Gemini models and its
Claude/GPT models separately. Before you start a chat on another harness or
pick a model for delegated work, check what is left:

```sh
orx agent quota            # quota left per account and model pool
orx agent quota --refresh  # ask the providers first (slower)
orx agent quota --json
```

- Send work to a harness and model whose pool has room (`ok`). Avoid `low`
  pools for long tasks and never target a `blocked` one; its row says when it
  comes back.
- On Antigravity, when the Claude & GPT pool is low or blocked, prefer a
  Gemini model (`--model gemini-…`), and the reverse.
- You do not pick the account: every turn runs on the account with the most
  room for its model and moves to another account if it hits a limit.
- When every account of every allowed harness is low, say so and keep large
  delegations for after the reset time the command shows.

When the user turned on the decision model (Settings → Harnesses), ask it
where new work should run before you pick a harness, model, or compute target:

```sh
orx agent route "<one-paragraph description of the work>"
orx agent route --stdin --json < brief.md
```

It answers with a harness, model, account, and compute target (and a GPU type
when one fits), each one of the options that are ready and have room, and a
one-line reason. Use its harness and model for `orx agent spawn --harness …
--model …` and its compute for the run, unless the user asked for something
else. When the decision model is off or unavailable the command still answers
from quota alone and says so.

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
- Do not split the experiment loop itself across harnesses, and keep ordinary literature
  retrieval local except for the `orx-deep-research` workflow.
