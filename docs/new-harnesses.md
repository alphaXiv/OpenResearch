# Kimi Code, MiniMax Code, and ZCode: integration notes

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
| Planned transport | ACP (`kimi acp`) | ACP (`mcode acp`) | print mode (`zcode -p … --json`) |

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

- `zcode app-server` runs a proprietary "ZCode Protocol" over stdio:
  `session/create`, `session/send`, `session/event` notifications,
  `session/setMode`, `session/setModel`, `session/resume`, `session/compact`,
  and so on. It is undocumented, and only a minified build is available.
- `zcode -p <prompt> --json` is the documented headless path. It supports
  `--resume sess_…`, `-c`, `--mode build|edit|yolo` (default `yolo` for
  `-p`), `--cwd`, `--attach`, and `--disallowed-tools`.
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
