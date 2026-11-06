# Contributing

## Before you start

- Check whether the issue already has an open pull request: `gh issue view <number> --json closedByPullRequestsReferences`. If it does, build on that one or review it; if yours would be better, say why there first. A second PR for the same issue wastes someone's work.
- Search open pull requests for the area too. A patch to code an open PR replaces gets closed as superseded.
- Link the issue the change fixes. Without an accepted issue, a patch may be declined on principle. An issue the patch's own author wrote does not count as accepted, and it does not bound the scope.

## Commits

- Title with one of three types and no scope. `scripts/change-counts.mjs` counts them for each release.
  - `feat:` implements a user-facing feature.
  - `fix:` fixes a user-facing or internal bug.
  - `chore:` is everything else: changes an OpenResearch update would not announce as a new feature or a fixed bug.
- After the type, name the change in a few words, like `feat: sort runs by duration`. The whole first line is at most 50 characters. The description carries the why.
- Credit each model with one trailer per model, like `Assisted-by: claude-opus-5.5` or `Assisted-by: gpt-6.1-sol`, on commit descriptions and pull request descriptions alike. Never add `Co-authored-by` for an agent, or a "Generated with Claude Code" line; `Assisted-by` replaces both and works across tools.
- Never guess what model you are. If you are not told, name the harness instead.

## Pull requests

- The title is the commit title, and the same rules apply.
- Say why and what is surprising. The reviewer reads the diff, so don't list hunks.
- Name the load-bearing choices, the ones that would be expensive to undo, so a reviewer can object before reading the rest.
- A new dependency, or a diff that adds far more than it removes, says why.
- A review finding left for later links the issue that tracks it.
- Review your agent's work before you file it. A patch that reads unreviewed, or documents a workaround instead of fixing it, gets closed.

## Evidence

- Every change a user can see comes with screenshots or a short video of the real flow, so a reviewer can judge it before checking out the code: before and after for a fix, every state for a dialog or a multi-step flow.
- Look at each frame before sending it: devtools left open, a stray scrollbar, an offset, the wrong flow.
- Say what you tested, where (which platforms, real or virtual machines, which agents), and what you did not.
- Attach screenshots and video with `--attach <file>` on `gh pr create`, `gh pr edit` or `gh pr comment`, after showing them to the user.

## Reviews and merging

- Don't merge. A PR is ready when CI is green and the review bots have nothing left; then say so and hand it back.
- Answer each bot or reviewer finding inline with what changed and how it was checked, or why it stays.

## Comments

- For maintainers (alphaXiv employees), present comments to the user before submitting them, unless they said to "post" them without mentioning "draft" or "prepare".
- On issues and pull requests, open each comment with a badge naming the model that wrote it: `![comment written by Claude Opus 5](https://img.shields.io/badge/comment_written_by-Claude_Opus_5-8a63d2?style=flat-square)`. Use the model's user-facing name, like `GPT-6 Astra` or `Claude Sonnet 4.5`, never its ID. In the badge URL, a space becomes `_`, `-` becomes `--` and `_` becomes `__`.
- Say exactly what was tested, and ask a bug reporter about their environment rather than assuming it.
