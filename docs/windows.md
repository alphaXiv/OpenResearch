# Windows

Windows support is in beta. The CLI and dashboard work, including local
experiment runs and the nanochat demo. The gaps are listed at the bottom.

## Prerequisites

**Git for Windows is required**, and for more than git. It is the only source of
the `bash` and coreutils that orx uses to run experiments — the `bash.exe` in
`System32` is the WSL launcher, which cannot see your files, and orx rejects it.
Install it with the standard installer so `git.exe` lands on `PATH`; orx finds
the shell by walking up from there.

You also need a coding agent. Claude Code is the default:

```powershell
winget install --id Git.Git -e
winget install --id OpenJS.NodeJS.LTS -e
npm install -g @anthropic-ai/claude-code
```

Open a new terminal afterwards so `PATH` is picked up.

The nanochat demo installs `uv`, and with it Python, on its first run.

## Install

### The desktop app

From [Releases](https://github.com/alphaXiv/OpenResearch/releases), download
`OpenResearch-Setup.exe` and run it. It installs for your account only, with no
administrator prompt, into `%LOCALAPPDATA%\Programs\OpenResearch`, adds
OpenResearch to the Start menu, and installs the Microsoft Edge WebView2 Runtime
if Windows lacks it. The dashboard opens in its own window; closing the window
quits OpenResearch, and starting it again while it runs brings the window back.
Running the installer or uninstaller while OpenResearch is open asks you to
close it first.

The install holds two programs. `OpenResearch.exe` is what the Start menu runs:
it starts `orx.exe app` in a hidden console, which `orx` and the git, shell, and
agent processes it starts share, so none of them flashes a window. Agents find
`orx` because its folder leads their `PATH`. `orx.exe` is the same binary as the
CLI release and updates itself the same way (below).

The app uses port 4792, or a free port if something else holds it.

### The CLI

From [Releases](https://github.com/alphaXiv/OpenResearch/releases), download
`openresearch-cli-x86_64-pc-windows-msvc.zip`, extract it, and double-click
`orx.exe`. It starts the dashboard at `http://127.0.0.1:4791` and opens your
browser. Leave the console window open — closing it stops the server. If orx
cannot start, a dialog says why.

To have `orx` on your `PATH` as a command instead, run the PowerShell installer,
which installs to `%USERPROFILE%\.cargo\bin`:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/alphaXiv/OpenResearch/releases/latest/download/openresearch-cli-installer.ps1 | iex"
```

Every install updates itself. `orx update`, or the Updates section of the
dashboard's Settings page, replaces `orx.exe` in place, and a running dashboard
offers a Restart button once the new version is on disk. The app's launcher
changes only with a new `OpenResearch-Setup.exe`.

### The SmartScreen warning

Official releases sign `orx.exe`, `OpenResearch.exe`, and `OpenResearch-Setup.exe`
with alphaXiv Inc.'s Azure Artifact Signing certificate. The signature identifies
the publisher and is timestamped so it remains valid after the short-lived
signing certificate expires. Older releases and PR test artifacts are unsigned.

Signing does not guarantee that "Windows protected your PC" disappears
immediately: [SmartScreen reputation](https://learn.microsoft.com/en-us/windows/msix/package/sign-msix-package-guide)
also depends on the app's download history. If a new release shows the warning,
check that **More info** identifies **alphaXiv Inc.** before choosing **Run anyway**.

### Long paths

Windows refuses paths over 260 characters. orx passes `core.longpaths` to git
itself, but a deep repository can still defeat the agent or your experiment
scripts. If you hit "path too long" from something that is not git, enable long
paths system-wide — once, as administrator, then reboot:

```powershell
Set-ItemProperty -Path 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' -Name LongPathsEnabled -Value 1
```

## Known gaps

| | |
|---|---|
| `orx up --remote-host` | Refused. The control channel is a Unix domain socket. |
| Restart after an update | There is no `exec`, so a restarting `orx up` starts a new process and exits. In a terminal the prompt comes back while the server keeps running in that console, where Ctrl+C still stops it; a supervisor sees the old process exit. |
| SSH connection reuse | Windows' OpenSSH cannot multiplex, so each status or log poll opens its own connection, and the Settings page uses the most recent preflight result instead of reporting a missing multiplexed master as a disconnection. Use a key held by an agent, or one without a passphrase. |
| The PATH guard | Not applied; it needs a POSIX shell startup file. |
| Data directory | Still `%USERPROFILE%\.local\share\openresearch`, not `%APPDATA%`. |
| Signing out or upgrading while the app runs | Windows ends the app at once, without stopping the agents it started or saving the last workspace state. |
| Starting the app while it is quitting | The new launch finds the old one still running and exits, so start it again once it has closed. |

## Releasing the app

`release-windows-app.yml` builds `OpenResearch-Setup.exe` from each release's
published `orx.exe` and attaches it, once the repository variable
`WINDOWS_APP_ENABLED` is `true`. Like the macOS app it follows a Release run
dispatched by a token (see `macos/DISTRIBUTION.md`); to attach the installer to
an existing release, dispatch the workflow with its tag. CI on every pull
request also uploads an `openresearch-windows-installer` artifact to test with.

### Release signing

Both `sign-windows-cli.yml` and `release-windows-app.yml` use the protected
`release-signing` environment. GitHub OIDC authenticates an Azure service
principal with the **Artifact Signing Certificate Profile Signer** role scoped
to the Public Trust certificate profile. No private key or client secret is
stored in GitHub. The federated credential subject must match the repository's
canonical name: `repo:alphaXiv/OpenResearch:environment:release-signing`.

Configure these environment variables before releasing:

| Variable | Value |
|---|---|
| `WINDOWS_SIGNING_CLIENT_ID` | Signing service principal's application ID |
| `WINDOWS_SIGNING_TENANT_ID` | Azure tenant containing that principal |
| `WINDOWS_SIGNING_SUBSCRIPTION_ID` | Subscription containing the signing account |
| `WINDOWS_SIGNING_ENDPOINT` | Signing account's regional endpoint |
| `WINDOWS_SIGNING_ACCOUNT` | Artifact Signing account name |
| `WINDOWS_SIGNING_PROFILE` | Public Trust certificate profile for alphaXiv Inc. |

The CLI archive is signed before cargo-dist generates the global checksums and
installers, so desktop self-updates retain a signed `orx.exe`. The app workflow
signs its executable payload before packaging, then signs the installer. Each
signing step requires a valid alphaXiv Inc. signature and timestamp before
uploading anything; a signing failure blocks publication.
