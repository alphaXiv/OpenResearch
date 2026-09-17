# DeepSeek Harness (DSH)

OpenResearch can drive [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) as a chat agent over ACP (`dsh --profile acp`).

## Requirements

1. Install `dsh` and put it on `PATH` (`dsh --version` should print).
2. Configure a DeepSeek API key the same way you do for the DSH web UI (`~/.dsh/.credentials.yaml` or `DEEPSEEK_API_KEY`).
3. In the OpenResearch model picker, choose **DeepSeek Harness**.

The first successful chat session loads the model list into a local cache. Refresh harness detection afterward if the picker is still empty.

## What works in this version

- Multi-turn chat with native DSH session resume after `orx up` restarts
- Tool calls (read / write / bash) streamed into the transcript
- Model and reasoning-effort selectors from DSH `configOptions`
- One-shot allow / reject permission cards when DSH asks

## Known limits

- No permission-mode dropdown (`read-only` / `workspace-write` / `danger-full-access`). Those come from DSH’s own settings / `DSH_PERMISSION_MODE`, not ACP.
- If `~/.dsh/settings.yaml` sets `permission.defaultPreset: danger-full-access`, DSH will not ask for approval. Change that preset in the DSH web UI if you want OpenResearch permission cards.
- No mid-turn steering, plan mode, or image input.
- On Linux, DSH may fall back from `bwrap` to landlock when unprivileged user namespaces are restricted.

## Logs

ACP stderr for the child process is written to `agent-dsh.log` under the OpenResearch data directory (`ORX_DATA_DIR`, or the default local store).
