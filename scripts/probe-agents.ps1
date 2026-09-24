# Probe the Kimi Code, MiniMax Code, and ZCode installs on this Windows machine
# so orx can find and drive them. It lists paths and file names, runs
# `--version` / `--help` on the agents' command-line tools, and opens an ACP
# handshake (initialize + session/new) in an empty temp folder. With -Turn it
# also sends each agent the single prompt "Reply with the single word OK." in
# that temp folder, over ACP and in print mode.
#
# Only files named kimi / mcode / zcode are ever run. Desktop apps (Kimi.exe,
# "MiniMax Code.exe", ZCode.exe) are never started, with one exception: ZCode's
# bundled runtime is run through ZCode.exe with ELECTRON_RUN_AS_NODE=1, which
# makes Electron behave as plain Node.js instead of opening the app. If a ZCode
# window appears anyway, that mode is disabled in this build; the probe closes
# it after the timeout and the report says so.
#
#   powershell -ExecutionPolicy Bypass -File scripts\probe-agents.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\probe-agents.ps1 -Turn
#
# The report is written to %TEMP%\orx-agent-probe.txt. Review it before
# sharing: it contains install paths and may contain your account name.
#
# Keep this file ASCII-only: Windows PowerShell 5.1 reads a BOM-less script in
# the ANSI code page, where UTF-8 punctuation can decode to a quote character.

param(
    [switch]$Turn,
    [int]$TimeoutSec = 90
)

$ErrorActionPreference = 'Continue'
$report = Join-Path $env:TEMP 'orx-agent-probe.txt'
$lines = New-Object System.Collections.Generic.List[string]
$maxCapture = 20000
function Say([string]$text) { $lines.Add($text); Write-Host $text }
function Clip([string]$text) {
    if ($null -eq $text) { return '' }
    if ($text.Length -le $maxCapture) { return $text }
    $text.Substring(0, $maxCapture) + "`n... [clipped $($text.Length - $maxCapture) chars]"
}

$userHome = $env:USERPROFILE
$agents = @(
    @{
        Id = 'kimi-code'; Name = 'kimi'
        Homes = @('.kimi-code')
        Dirs = @('.kimi-code', '.local\bin', 'AppData\Local\Programs\kimi-code', 'AppData\Local\kimi-code')
        Print = @('-p', 'Reply with the single word OK.', '--output-format', 'stream-json')
        Acp = $true
    },
    @{
        Id = 'minimax-code'; Name = 'mcode'
        Homes = @('.minimax', '.minimax-code')
        Dirs = @('.minimax-code', '.local\bin', '.minimax\bin')
        Print = @('exec', '--output-format', 'stream-json', 'Reply with the single word OK.')
        Acp = $true
    },
    @{
        Id = 'zcode'; Name = 'zcode'
        Homes = @('.zcode')
        Dirs = @('.zcode\runtime', '.local\bin')
        Print = @('-p', 'Reply with the single word OK.', '--json', '--no-color')
        Acp = $false
    }
)

$npmPrefix = $null
if (Get-Command npm -ErrorAction SilentlyContinue) {
    $npmPrefix = (& npm prefix -g 2>$null | Select-Object -First 1)
}

function Quote-Arg([string]$a) {
    if ($a -eq '') { return '""' }
    if ($a -notmatch '[\s"]') { return $a }
    '"' + ($a -replace '"', '\"') + '"'
}

# A ProcessStartInfo that runs `exe` with `argv`: .cmd through cmd.exe, .ps1
# through powershell.exe, anything else directly.
function New-Psi([string]$exe, [string[]]$argv, [string]$cwd, [hashtable]$envs) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $joined = ($argv | ForEach-Object { Quote-Arg $_ }) -join ' '
    if ($exe -match '\.(cmd|bat)$') {
        $psi.FileName = $env:ComSpec
        $psi.Arguments = '/d /s /c "' + (Quote-Arg $exe) + ' ' + $joined + '"'
    } elseif ($exe -match '\.ps1$') {
        $psi.FileName = 'powershell.exe'
        $psi.Arguments = '-NoProfile -ExecutionPolicy Bypass -File ' + (Quote-Arg $exe) + ' ' + $joined
    } else {
        $psi.FileName = $exe
        $psi.Arguments = $joined
    }
    $psi.WorkingDirectory = $cwd
    $psi.UseShellExecute = $false
    $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = $true
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8
    if ($envs) { foreach ($k in $envs.Keys) { $psi.EnvironmentVariables[$k] = $envs[$k] } }
    $psi
}

function Stop-Tree($p) {
    try { & taskkill.exe /PID $p.Id /T /F 2>$null | Out-Null } catch {}
    try { if (-not $p.HasExited) { $p.Kill() } } catch {}
}

function Invoke-Capture([string]$exe, [string[]]$argv, [string]$cwd, [int]$timeout, [hashtable]$envs) {
    $p = [System.Diagnostics.Process]::Start((New-Psi $exe $argv $cwd $envs))
    $p.StandardInput.Close()
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit($timeout * 1000)) {
        Stop-Tree $p
        return Clip ("TIMEOUT after ${timeout}s`nstdout:`n" + $out.Result + "`nstderr:`n" + $err.Result)
    }
    $p.WaitForExit()
    Clip ("exit $($p.ExitCode)`nstdout:`n" + $out.Result + "`nstderr:`n" + $err.Result)
}

# One ACP exchange over stdio: initialize, session/new, and (with -Turn) a
# single session/prompt. Every line the agent writes is recorded verbatim.
# Permission requests are always cancelled: the probe never lets an agent act.
function Invoke-Acp([string]$exe, [string]$cwd, [int]$timeout, [bool]$turn) {
    $p = [System.Diagnostics.Process]::Start((New-Psi $exe @('acp') $cwd $null))
    $errTask = $p.StandardError.ReadToEndAsync()
    $log = New-Object System.Collections.Generic.List[string]
    $deadline = (Get-Date).AddSeconds($timeout)
    $script:pending = $null

    function Send($id, $method, $params) {
        $msg = @{ jsonrpc = '2.0'; id = $id; method = $method; params = $params } | ConvertTo-Json -Depth 20 -Compress
        $log.Add(">> $msg")
        $p.StandardInput.WriteLine($msg)
        $p.StandardInput.Flush()
    }
    function Await($id) {
        while ((Get-Date) -lt $deadline) {
            if ($null -eq $script:pending) { $script:pending = $p.StandardOutput.ReadLineAsync() }
            if (-not $script:pending.Wait(500)) { continue }
            $line = $script:pending.Result
            $script:pending = $null
            if ($null -eq $line) { $log.Add('<< EOF'); return $null }
            $log.Add("<< $line")
            try { $m = $line | ConvertFrom-Json } catch { continue }
            if ($m.method -eq 'session/request_permission' -and $null -ne $m.id) {
                $reply = @{ jsonrpc = '2.0'; id = $m.id; result = @{ outcome = @{ outcome = 'cancelled' } } } | ConvertTo-Json -Depth 10 -Compress
                $log.Add(">> $reply")
                $p.StandardInput.WriteLine($reply)
                $p.StandardInput.Flush()
                continue
            }
            if ($null -ne $m.id -and $m.id -eq $id -and ($null -ne $m.result -or $null -ne $m.error)) { return $m }
        }
        $log.Add('<< TIMEOUT')
        $null
    }

    Send 1 'initialize' @{ protocolVersion = 1; clientCapabilities = @{ fs = @{ readTextFile = $false; writeTextFile = $false }; terminal = $false }; clientInfo = @{ name = 'orx-probe'; version = '0' } }
    $init = Await 1
    if ($init) {
        Send 2 'session/new' @{ cwd = $cwd; mcpServers = @() }
        $new = Await 2
        if ($turn -and $new -and $new.result.sessionId) {
            Send 3 'session/prompt' @{ sessionId = $new.result.sessionId; prompt = @(@{ type = 'text'; text = 'Reply with the single word OK.' }) }
            $null = Await 3
        }
    }
    try { $p.StandardInput.Close() } catch {}
    if (-not $p.WaitForExit(5000)) { Stop-Tree $p }
    $log.Add("stderr:`n" + (Clip $errTask.Result))
    $log -join "`n"
}

function Find-Cli($agent) {
    $found = New-Object System.Collections.Generic.List[string]
    Get-Command $agent.Name -All -ErrorAction SilentlyContinue |
        Where-Object { $_.Source } | ForEach-Object { $found.Add($_.Source) }
    $dirs = @($agent.Dirs | ForEach-Object { Join-Path $userHome $_ })
    if ($npmPrefix) { $dirs += $npmPrefix }
    foreach ($dir in $dirs) {
        if (-not (Test-Path $dir)) { continue }
        Get-ChildItem $dir -Recurse -Depth 4 -File -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match "^$($agent.Name)\.(exe|cmd|ps1)$" } |
            ForEach-Object { $found.Add($_.FullName) }
    }
    @($found | Select-Object -Unique)
}

# Prefer .exe, then .cmd, then .ps1: the order orx itself will use.
function Pick-Cli([string[]]$paths) {
    foreach ($ext in @('.exe', '.cmd', '.ps1')) {
        $hit = $paths | Where-Object { $_.ToLower().EndsWith($ext) } | Select-Object -First 1
        if ($hit) { return $hit }
    }
    $null
}

function Show-Home([string]$dir, [int]$depth) {
    if (-not (Test-Path $dir)) { Say "  $dir : missing"; return }
    Say "  $dir (names only):"
    Get-ChildItem $dir -Force -Recurse -Depth $depth -ErrorAction SilentlyContinue |
        Select-Object -First 80 |
        ForEach-Object { Say "    $($_.FullName.Substring($dir.Length))$(if ($_.PSIsContainer) { '\' })" }
}

Say "orx agent probe v2 - $(Get-Date -Format o)"
Say "Windows: $([Environment]::OSVersion.VersionString); PowerShell $($PSVersionTable.PSVersion)"
foreach ($tool in @('git', 'bash', 'node', 'npm')) {
    $c = Get-Command $tool -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($c) { Say "$tool : $($c.Source)" } else { Say "$tool : not on PATH" }
}
if (Get-Command git -ErrorAction SilentlyContinue) { Say "git --version: $(& git --version 2>&1)" }
foreach ($gb in @("$env:ProgramFiles\Git\bin\bash.exe", "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe")) {
    Say "Git Bash at ${gb}: $(Test-Path $gb)"
}
Say "npm global prefix: $npmPrefix"
Say ''

$work = Join-Path $env:TEMP ("orx-probe-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $work | Out-Null
if (Get-Command git -ErrorAction SilentlyContinue) { & git -C $work init -q 2>$null }

foreach ($agent in $agents) {
    Say "=================== $($agent.Id)"
    Say 'config homes:'
    foreach ($h in $agent.Homes) { Show-Home (Join-Path $userHome $h) 1 }
    if ($agent.Id -eq 'minimax-code') { Show-Home (Join-Path $userHome '.minimax\auth') 2 }
    if ($agent.Id -eq 'kimi-code') {
        $daimon = Join-Path $env:APPDATA 'kimi-desktop\daimon-bundle'
        if (Test-Path $daimon) {
            Say "kimi desktop daimon bundle: $daimon"
            Get-ChildItem $daimon -Force -ErrorAction SilentlyContinue | ForEach-Object { Say "    $($_.Name)$(if ($_.PSIsContainer) { '\' })" }
            $cmd = Join-Path $daimon 'bin\kimi-daimon.cmd'
            if (Test-Path $cmd) { Say "  kimi-daimon.cmd:"; Get-Content $cmd | ForEach-Object { Say "    $_" } }
        }
    }

    $clis = @(Find-Cli $agent)
    Say 'command-line candidates:'
    if ($clis.Count -eq 0) { Say '  (none)' } else { $clis | ForEach-Object { Say "  $_" } }
    $bin = Pick-Cli $clis
    if ($bin) {
        Say "using: $bin"
        Say '--- --version'
        Say (Invoke-Capture $bin @('--version') $work 30 $null)
        Say '--- --help'
        Say (Invoke-Capture $bin @('--help') $work 30 $null)
        if ($agent.Acp) {
            Say '--- ACP'
            Say (Invoke-Acp $bin $work $TimeoutSec ([bool]$Turn))
        }
        if ($Turn) {
            Say '--- print mode turn'
            Say (Invoke-Capture $bin $agent.Print $work $TimeoutSec $null)
        }
    }

    if ($agent.Id -eq 'zcode') {
        $app = Join-Path $env:LOCALAPPDATA 'Programs\ZCode\ZCode.exe'
        $cjs = Join-Path $env:LOCALAPPDATA 'Programs\ZCode\resources\glm\zcode.cjs'
        Say "bundled runtime: ZCode.exe=$(Test-Path $app) zcode.cjs=$(Test-Path $cjs)"
        $resources = Join-Path $env:LOCALAPPDATA 'Programs\ZCode\resources'
        if (Test-Path $resources) {
            Say "  $resources (two levels, names only):"
            Get-ChildItem $resources -Force -Recurse -Depth 1 -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -notmatch 'app\.asar\.unpacked\\node_modules' } |
                Select-Object -First 80 |
                ForEach-Object { Say "    $($_.FullName.Substring($resources.Length))$(if ($_.PSIsContainer) { '\' })" }
        }
        # Outside the Desktop app the runtime needs its built-in provider
        # catalog passed explicitly; the npm repack ships it as provider/.
        $builtin = @(
            (Join-Path $env:LOCALAPPDATA 'Programs\ZCode'),
            (Join-Path $userHome '.zcode')
        ) | Where-Object { Test-Path $_ } | ForEach-Object {
            Get-ChildItem $_ -Recurse -Depth 6 -File -Filter 'zcode-builtin.json' -ErrorAction SilentlyContinue
        } | Select-Object -ExpandProperty FullName
        Say 'zcode-builtin.json candidates:'
        if ($builtin) { $builtin | ForEach-Object { Say "  $_" } } else { Say '  (none)' }
        if ((Test-Path $app) -and (Test-Path $cjs)) {
            $asNode = @{ ELECTRON_RUN_AS_NODE = '1' }
            if ($builtin) { $asNode['ZCODE_BUILTIN_PROVIDER_CONFIG_FILE'] = @($builtin)[0] }
            Say '--- bundled: ZCode.exe zcode.cjs --version (ELECTRON_RUN_AS_NODE=1)'
            Say (Invoke-Capture $app @($cjs, '--version') $work 30 $asNode)
            Say '--- bundled: ZCode.exe zcode.cjs --help'
            Say (Invoke-Capture $app @($cjs, '--help') $work 30 $asNode)
            if ($Turn) {
                Say '--- bundled: print mode turn'
                Say (Invoke-Capture $app (@($cjs) + $agent.Print) $work $TimeoutSec $asNode)
            }
        }
    }
    Say ''
}

Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
$lines | Set-Content -Path $report -Encoding UTF8
Write-Host ''
Write-Host "Report written to $report - review it, then share it."
