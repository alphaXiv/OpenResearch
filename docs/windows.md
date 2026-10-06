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

From [Releases](https://github.com/artur-shaikhutdinov/OpenResearch-Kimi-MiniMax-ZCode/releases), download
`openresearch-cli-x86_64-pc-windows-msvc.zip`, extract it, and double-click
`orx.exe`. It starts the dashboard at `http://127.0.0.1:4791` and opens your
browser. Leave the console window open — closing it stops the server. If orx
cannot start, a dialog says why.

To have `orx` on your `PATH` as a command instead, run the PowerShell installer,
which installs to `%USERPROFILE%\.cargo\bin`:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/artur-shaikhutdinov/OpenResearch-Kimi-MiniMax-ZCode/releases/latest/download/openresearch-cli-installer.ps1 | iex"
```

Either install updates itself. `orx update`, or the Updates section of the
dashboard's Settings page, replaces `orx.exe` in place, and a running dashboard
offers a Restart button once the new version is on disk.

### The SmartScreen warning

This fork's `orx.exe` is not code-signed, so Windows shows "Windows protected
your PC" on first run. Choose **More info** → **Run anyway**. (alphaXiv's own
releases are signed with alphaXiv Inc.'s certificate; this fork cannot use it.)

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

This fork publishes only the `orx` CLI for Windows. alphaXiv's desktop installer
(`OpenResearch-Setup.exe`) and its signing need alphaXiv's signing account;
`release-windows-app.yml` and `sign-windows-cli.yml` stay unused here, since
`WINDOWS_APP_ENABLED` is not set and this fork's `release.yml` does not call the
signing job.
