# Local models with OpenCode

OpenResearch uses OpenCode for file edits, commands, permissions, and the agent
loop. LM Studio, oMLX, or Ollama serves the model on your computer. An
OpenAI-compatible API is a protocol; it does not require an OpenAI account.

## Start a server

- **LM Studio:** install from https://lmstudio.ai, download a tool-capable model,
  load it, and start the local server: `http://127.0.0.1:1234/v1` by default.
- **oMLX (Apple Silicon):** install from https://github.com/jundot/omlx,
  download a tool-capable MLX model, and start the server:
  `http://127.0.0.1:8000/v1` by default.
- **Ollama:** install from https://ollama.com, download a tool-capable model,
  and start the server: `http://127.0.0.1:11434/v1` by default.

Use the model ID returned by `GET /v1/models`. Choose a model and context window
that fit your memory. Start with at least 16K context when the model and hardware
permit; ordinary chat support does not guarantee reliable coding tools.

## Configure OpenCode

Install OpenCode from https://opencode.ai. Merge this into its global
`~/.config/opencode/opencode.json` (or `$XDG_CONFIG_HOME/opencode/opencode.json`).
Global configuration is needed because OpenResearch creates session worktrees
and runs auxiliary requests outside your project directory.

Replace all three occurrences of `YOUR_MODEL_ID`. Adjust the server URL and
context/output limits to your model. This example restricts OpenCode to the local
provider, including its defaults and auxiliary requests.

```json
{
  "$schema": "https://opencode.ai/config.json",
  "enabled_providers": ["local"],
  "model": "local/YOUR_MODEL_ID",
  "small_model": "local/YOUR_MODEL_ID",
  "share": "disabled",
  "provider": {
    "local": {
      "npm": "@ai-sdk/openai-compatible",
      "name": "Local models",
      "options": { "baseURL": "http://127.0.0.1:1234/v1" },
      "models": {
        "YOUR_MODEL_ID": {
          "name": "My local model",
          "limit": { "context": 16384, "output": 4096 }
        }
      }
    }
  }
}
```

OpenResearch recognizes models declared under `provider.<id>.models` and checks
the loopback server's model list without running inference. If the server requires
authentication, set `options.apiKey` in OpenCode: this readiness check does not
read keys stored by `opencode auth login`. Remote servers retain the normal
provider authentication flow. The example disables OpenCode sharing separately
so local conversations are not published through its sharing feature.

## Onboarding

1. Open onboarding and re-check installed agents.
2. Select **OpenCode**, then your local model in the model selector.
3. Complete onboarding and send a task. Existing installations can re-check
   OpenCode in Settings and select the model when starting a session.

No cloud account is required. If the server is unavailable or the configured
model is missing, start/load it and re-check. Requests to the selected model
fail rather than switching to a cloud model.

Local inference does not mean the research workflow is offline. Downloads,
paper searches, GitHub operations, connected tools, and agent commands may use
the network. OpenResearch's usage analytics setting is separate from model
routing. Configure these separately for an offline workflow.
