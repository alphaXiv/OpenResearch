# Local models with OpenCode

OpenCode runs the agent loop: file edits, commands, and tools. LM Studio, oMLX,
or Ollama serves the model on your computer. No cloud account is required for
local models. Your existing OpenCode providers remain available alongside them.

## Connect from OpenResearch

Choose **Add Local Model** under OpenCode in the model picker, or open
**Settings → Harnesses → OpenCode → Add local model**. Onboarding shows
compatibility; configuration happens in the add-model dialog.

1. Choose your model app. Install OpenCode if the setup screen says it is missing.
2. Follow the model app's setup instructions below. Download a model that supports
   tool calling, load it, and start its local server.
3. In OpenResearch, check the server address and select **Find models**.
   Expand **Authentication** only if your server requires a credential.
4. Select a model from the server's list. Match the context window to the value
   loaded in your model app; 32K is a useful starting point for agent tools.
5. Select **Save model**, then select it under OpenCode in the model picker.
   An existing chat keeps its harness; start a new chat to switch harnesses.

OpenResearch saves these connections in its local data directory, not in your
OpenCode configuration files. Existing provider settings and credentials are
preserved. Connecting another model from the same server adds it to that
connection. Disconnecting in Settings removes the connection from new chats;
it does not stop the model app or delete downloaded models.

## LM Studio

Install from https://lmstudio.ai and complete its desktop setup. Enable
**Developer Mode**. If you already downloaded a model, you can skip the suggested
first download and use that model instead.

Choose **Load Model**, enable **Manually choose model load parameters**, select
a tool-capable model, and set **Context Length** to `32768` when memory permits.
Then open **Developer → Local Server** and turn the server on. The default
OpenResearch address is `http://127.0.0.1:1234/v1`.

Keep LM Studio running while using its model. LM Link and cloud login are not
required for a server on the same computer.

## oMLX (Apple Silicon)

Install from https://github.com/jundot/omlx. Put a tool-capable MLX model in its
model directory, start the server, and keep it running. The default address is
`http://127.0.0.1:8000/v1`. oMLX can load a discovered model on its first request.

## Ollama

Install from https://ollama.com, download a model with tool support, and keep
Ollama running. The default address is `http://127.0.0.1:11434/v1`. This provider
uses the same OpenAI-compatible connection flow.

## Other OpenAI-compatible servers

Choose **Custom Endpoint (OpenAI compatible)** for a server such as vLLM. It must expose
`GET /v1/models` and tool-capable chat completions. Enter its base URL ending in
`/v1`, and its API key under **Authentication** if required.

The address must be loopback, relative to the machine running OpenResearch.
With OpenResearch's SSH remote-host mode, run the model server on that remote
machine and use `http://127.0.0.1:8000/v1`. For agents running on your Mac with
inference on another machine, forward the model server over SSH and enter the
forwarded local address instead.

## Troubleshooting and privacy

- **Cannot reach the server:** start it in the model app and verify the address.
- **No models:** download/load a model, then check again.
- **Authentication required:** enter the local server's API key.
- **Saved but missing from OpenCode:** re-check installed agents. An explicit
  OpenCode provider allowlist or administrator policy can exclude the new
  connection; those restrictions are not silently removed.
- **Context errors:** match the context setting to the model app's loaded window.
  A small model or short context may not reliably handle coding tools.

Models added through OpenResearch also handle OpenCode's auxiliary title requests, and
OpenCode sharing is disabled for those local sessions. A local server failure
fails or retries that request; it does not select a cloud model instead.

Local inference does not mean the research workflow is offline. Downloads,
paper searches, GitHub operations, connected tools, and agent commands may use
the network. OpenResearch's usage analytics setting is separate from model
routing.

Existing manually configured loopback providers in OpenCode remain supported.
Declare their models and `options.baseURL` in OpenCode's global configuration;
if authentication is required, use `options.apiKey` for readiness checks.
