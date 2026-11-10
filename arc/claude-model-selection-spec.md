# Custom Claude model selection

## Goal

Keep a selected external model when Claude Code changes permission modes.

## Scope

Resolve model aliases at the resident Claude process boundary. Cover launch, resume, restart, compaction, and live model control.

## Required behavior

- Use Claude Code's `list_models` response in the chat worktree and prepared native home.
- When a native alias resolves to an external model ID, send that explicit ID to Claude Code.
- Keep the saved picker selection unchanged.
- Preserve native `claude-*` alias behavior, intentional `opusplan` behavior, explicit IDs, and null defaults.
- Reuse an unchanged resident child without another catalog process.
- Return a specific error when catalog resolution fails. Do not silently substitute another model.
- Keep different chats independent. Do not write model selections into shared settings files.

## Exclusions

This change does not add a provider, edit user configuration, change the picker layout, alter native plan approval, or change other harnesses.

## Constraints

Use the native catalog rather than duplicate Claude configuration precedence. Add no shared cache or dependency. Keep the saved selection separate from launch and control values without unused stored state.

Tests must set temporary HOME, USERPROFILE, XDG, and native agent paths before native-store access. Check effective path precedence and an external settings sentinel. Test through real module interfaces with a fake external CLI.

## Acceptance checks

1. An existing `haiku` selection that resolves to `gpt-6-astra` launches the child with `gpt-6-astra`.
2. The same selection rule applies to resume, restart, compaction, and live `set_model` requests.
3. Plan-mode changes retain the selected external model instead of the native alias's mode-dependent model.
4. Native aliases, `opusplan`, defaults, and explicit IDs keep their existing behavior.
5. An unchanged child needs no new catalog process. Concurrent chats keep separate selections.
6. Shared configuration contents and symlink targets stay unchanged.
7. Catalog denial, invalid responses, missing resolutions, timeout, and spawn failures reach the production chat error with their specific cause.
8. The regression test rejects the old alias-only behavior.

## Implementation plan

1. Resolve external alias mappings before process launch and live model control.
2. Keep saved selections unchanged after successful model control.
3. Test process paths, compatibility, chat separation, isolation, and failures.
4. Run formatting, Clippy, Rust tests, and the relevant dashboard checks.
5. Have a fresh reviewer check the final diff and test evidence against this specification.
