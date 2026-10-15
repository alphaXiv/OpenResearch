# Kimi Code, MiniMax Code, and ZCode: integration notes

## Using them

`orx up` detects each agent and lists it under Settings → Harnesses.

- **Kimi Code**: install the CLI (Windows:
  `irm https://code.kimi.com/kimi-code/install.ps1 | iex`; macOS/Linux:
  `curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash`), then run
  `kimi login`. The Kimi desktop app is not used. Modes: Ask, Auto, YOLO, and
  `/plan`. Pick a model your plan includes: a model outside it is refused
  with the provider's reason (for example K2.7 Code Highspeed on a basic plan).
- **MiniMax Code**: install the CLI from the dashboard, or run MiniMax's
  installer (`irm https://filecdn.minimax.chat/public/install.ps1 | iex`).
  When its final step fails, the dashboard's installer falls back to
  `npm install` with the Node.js the installer provided, into
  `~/.minimax-code/npm`. Sign in with `mcode login --region global` (or
  `--region cn` for a mainland account). Modes: Auto, Full access, and
  `/plan`. There is no Ask: MiniMax's own `default` mode ran commands outside
  the project without asking.
- **ZCode**: uses the runtime bundled with the ZCode desktop app (or a `zcode`
  CLI on PATH), driven through `zcode app-server --stdio`, the protocol the
  ZCode app itself uses. Modes map onto ZCode's own:
  - Ask is `build`: it asks before changes and commands.
  - Edit is `edit`: file edits run, and it asks before commands.
  - Auto (the default) is `guarded`: it asks only when there is risk.
  - Full access is `yolo`.
  - Plan (in the Mode menu, or `/plan`) is ZCode's read-only `plan` mode.

  ZCode before 3.15 has no `guarded` mode, so Auto runs in `edit` there.

  Approvals, AskUserQuestion and ExitPlanMode become orx's permission,
  question and plan cards while the turn waits. An approved plan continues the
  same turn in the composer's mode. Each chat runs on its own model
  (`session/setModel`); the ZCode app's default model is left alone.
  - The composer's Provider menu switches between Coding Plan and API key;
    the Model menu then lists that provider's models. Both are offered even
    before they have models: one without models opens Settings → Agents,
    whose ZCode card shows each provider's state and what it needs.
  - Coding Plan: signing in to the app is not enough. Outside the app the
    runtime needs its own sign-in, `zcode login` (Settings → Harnesses →
    ZCode → Sign in). Since ZCode 3.15 it saves an OAuth token
    (`auth-source` `oauth`); earlier versions saved the Coding Plan key. It
    covers the Z.ai Individual Coding Plan.
    The app's runtime runs its app-server as the app's helper: the app
    supplies the account. orx starts it through a small loader
    (`src/local/harness/zcode_app_server.cjs`) that, in memory, makes the two
    edits of the npm repack's `app-server-standalone-auth`: it starts the
    provider registry the way `zcode -p` does, reading `zcode login`'s
    sign-in, and gives that registry's request credentials to each session
    (without the second edit, each model request still asks the host). The
    loader logs `orx: ZCode app-server sign-in: patched|native|unavailable`.
    On a build where it finds nothing to change, API-key providers still
    work, and a Coding Plan turn says so.
  - API key: add a provider with a key in the ZCode app (Settings →
    Providers: Z.ai API billed from the balance, Z.ai Coding Plan by key, or
    a custom one), then Refresh the agent in orx. The app keeps the key in
    `~/.zcode/v2/provider_config.json` (`config.access.apiKey`), where the
    runtime reads it; orx stores no key of its own, as for the other agents,
    and lists the provider's models under API key.
  - Start Plan works only inside the app: its endpoint
    (`zcode.z.ai/api/v1/zcode-plan`) needs a captcha token the app obtains, and
    the Coding Plan endpoints answer `1113` for a Start Plan account.
  - On Windows `zcode login` opens the browser through `cmd /c start`, which
    cuts the URL at its first `&`; orx runs `zcode login --no-browser` through
    PowerShell, which pipes its output (ZCode.exe is a GUI program and prints
    nothing in a terminal) and opens the printed URL.
  - With no usable provider a turn fails with "Select a model before
    continuing"; a provider without a plan or balance fails with `1113`.
  - `project_token_login_failed`: Z.ai refused the token `zcode login` saved
    (the runtime trades it at `api.z.ai/api/auth/z/login` for a project
    token). orx cannot tell offline, so ZCode still shows as signed in; the
    turn says to sign in again, and the ZCode card has Sign in again.
  - The runtime uses a plan only when its API key in `credentials.json` is
    saved under the identity `zcode login` recorded. Signing in to the app
    with another account replaces the key, and turns fail with "Select a
    model"; orx then shows ZCode as not signed in and asks for `zcode login`.
  - The composer lists the models of each signed-in plan (from the app's
    built-in provider catalog) and of each API-key provider (the models its
    template enables).

The rest of this file is the integration notes.

Working notes for adding three harnesses beside Claude Code, Codex, OpenCode,
Cursor, and Antigravity (see `src/local/harness/mod.rs`). Each fact below was
observed against the npm builds listed here, in a Linux container with no
account signed in. Windows install paths come from `scripts/probe-agents.ps1`.

| | Kimi Code | MiniMax Code | ZCode |
|---|---|---|---|
| npm package (observed) | `@moonshot-ai/kimi-code` 2.1.1 | `@minimax-ai/code` 0.5.4 | `zcode-app-cli` 3.14.3-27 (runtime 0.16.9; unofficial repack of the Desktop runtime) |
| Binary | `kimi` | `mcode` | `zcode` |
| Windows installer | `irm https://code.kimi.com/kimi-code/install.ps1 \| iex` | official PowerShell installer or npm | Desktop app; official CLI installs to `~/.zcode/runtime` |
| Config home | `~/.kimi-code` | `~/.minimax` (`MINIMAX_DATA_DIR`) | `~/.zcode` (`ZCODE_DATA_BASE_DIR`); providers in `~/.zcode/v2/provider_config.json` |
| Login | `kimi login` (device code; `--region global`) | `mcode login [--region global]` | `zcode login` (Z.AI OAuth) |
| Transport | ACP (`kimi acp`) | ACP (`mcode acp`) | ZCode Protocol (`zcode app-server --stdio`) |

## ACP (Kimi, MiniMax)

Both speak Agent Client Protocol v1: JSON-RPC 2.0, one message per line on
stdio.

- `initialize` succeeds without an account. Both report `loadSession: true`,
  and `sessionCapabilities` includes `list`, `resume`, `close`, and `fork`.
  Kimi accepts images and embedded context. MiniMax accepts neither.
- `session/new {cwd, mcpServers: []}` without an account returns
  `{"code":-32000,"message":"Authentication required…"}`. Detection can use
  this as its signed-out signal.
- Kimi's `authMethods` include a `terminal-auth` entry (`kimi login`), which
  can back the dashboard's login action.
- MiniMax advertises extensions under `_meta["minimax-code/extensions"]`,
  including `mcode/session/steer` and a message queue. Those are the steering
  hooks.
- `mcode exec --permission` supports only `smart|full|off`. "Ask" needs ACP,
  which is one more reason to use ACP here.

Print-mode fallbacks exist if ACP turns out to be unsuitable:
- `kimi -p <prompt> --output-format stream-json`, with `-S <id>` to resume,
  `--plan`, `--yolo` / `--auto`, `--skills-dir`, and `--agent-file`.
- `mcode exec --output-format stream-json --cwd … --session <id> --model
  provider/model --effort <level>`.

## ZCode

- `zcode app-server --stdio` speaks the ZCode Protocol: JSON-RPC-shaped
  NDJSON without a `jsonrpc` field (a message carrying one is rejected).
  Its schema is `packages/shared/src/zcode-protocol/index.ts` in
  [zai-org/ZCode](https://github.com/zai-org/ZCode). A turn, as recorded in
  `src/local/harness/fixtures/zcode_app_server_*.jsonl`:
  1. `session/create {workspace, mode}` or `session/resume {sessionId,
     workspace}` returns a snapshot. Its `settings` holds the current and
     available models (with reasoning levels) and the mode.
  2. `session/setMode`, then `session/setModel {model: {providerId, modelId,
     options: {reasoningLevel}}, persistAsWorkspaceLastUsed: false}`. A model
     with reasoning levels is refused without one.
  3. `session/subscribe {deliveryKind: "desktop-continuous"}`, then
     `session/send {content}`. Events arrive as `session/event` notifications,
     the same events print mode streams, up to `turn.completed` or
     `turn.failed`.
  4. The server asks back:
     - `session/requestRuntimePreferences` (answered with
       `askUserQuestionAutoResolutionEnabled: false`, so questions reach the
       user);
     - `interaction/requestPermission` (answered with one option's
       `response`, `{decision, reason}`);
     - `interaction/requestUserInput` for AskUserQuestion (`{action: "accept",
       content: {answers: {question: label}}}`) and for plan approval
       (`schema.interaction: "plan_approval"`, approved with `{answer:
       "approve"}`, any other answer is feedback).

     While it waits, ZCode re-announces a request with a new id and the
     same `requestId`.
  5. `session/stop` ends the turn with `turn.completed` (`resultType:
     "cancelled"`). orx instead closes the process, and the next turn resumes.
- `zcode -p <prompt> --output-format stream-json` is the headless print
  mode orx used before. It has no approval channel and no `--model`.
- Without a provider, it exits 1 with `Model creation failed`.
- **Open question:** does the Windows Desktop app ship a runnable `zcode`
  CLI, or only an Electron bundle? `probe-agents.ps1` answers this.

## Windows desktop apps (first probe report)

The desktop apps do not put a command-line tool on PATH:

- **Kimi**: `%LOCALAPPDATA%\Programs\Kimi\Kimi.exe` (Electron 3.2.x) plus
  `%APPDATA%\kimi-desktop\daimon-bundle`. It does not use `~/.kimi-code`, so
  the desktop login is not shared with Kimi Code CLI. orx drives the official
  CLI (`irm https://code.kimi.com/kimi-code/install.ps1 | iex`), and the user
  signs in once with `kimi login`.
- **MiniMax Code**: `%LOCALAPPDATA%\Programs\MiniMax Code\MiniMax Code.exe`
  (Electron, agent inside `app.asar`). It keeps its data in `~/.minimax`
  (`auth\`, `sessions\`, `config.yaml`), the same directory `mcode` uses. orx
  drives the official CLI (`irm https://filecdn.minimax.chat/public/install.ps1
  | iex`, launchers in `%USERPROFILE%\.minimax-code`). The desktop login is
  expected to carry over; this is not yet verified.
  On the test machine the installer failed at activation ("Staged versioned
  MCode release validation failed") after it had provisioned Node in
  `%USERPROFILE%\.minimax-code\runtime\node-v22.19.0-win-x64`. Installing
  with that Node's npm (`npm install -g --prefix
  %USERPROFILE%\.minimax-code\npm @minimax-ai/code`) works. The resulting
  `mcode.cmd` needs that Node on PATH, so orx must prepend it when spawning.
  `mcode login` defaults to the `cn` region (account.minimax.cn). Accounts
  created in the international desktop app need `mcode login --region
  global`.
- **ZCode**: `%LOCALAPPDATA%\Programs\ZCode\ZCode.exe` bundles the CLI runtime
  as `resources\glm\zcode.cjs`, the same file the npm repack ships. orx can run
  it as `ZCode.exe resources\glm\zcode.cjs …` with `ELECTRON_RUN_AS_NODE=1`.
  Outside the app, the runtime also needs `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE`
  pointing at `zcode-builtin.json`. Without it, `-p` fails with "无法定位 CLI
  ZCode Built-in Provider Config". Whether this build's Electron fuses allow
  RunAsNode is still to be confirmed on Windows.

Never launch the GUI executables to probe them. Only files named
`kimi`/`mcode`/`zcode` are command-line tools.

## Still to capture

Real turn streams (text, tool calls, permission requests, errors) for the
unit-test fixtures. Two ways to get them:
- `probe-agents.ps1 -Turn` on a signed-in machine;
- a local OpenAI-compatible mock provider registered through each CLI's custom
  provider support.
