# Probe the Kimi Code, MiniMax Code, and ZCode installs on this Windows machine
# so orx can find and drive them. Read-only: it lists paths and file names,
# runs `--version` / `--help`, and opens an ACP handshake (initialize +
# session/new in an empty temp folder). No prompt is sent unless -Turn is
# given, and then only "Reply with the single word OK." in that temp folder.
#
#   powershell -ExecutionPolicy Bypass -File scripts\probe-agents.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\probe-agents.ps1 -Turn
#
# The report is written to %TEMP%\orx-agent-probe.txt. Review it before
# sharing: it contains install paths and may contain your account name.

param(
    [switch]$Turn,
    [int]$TimeoutSec = 60
)

$ErrorActionPreference = 'Continue'
$report = Join-Path $env:TEMP 'orx-agent-probe.txt'
$lines = New-Object System.Collections.Generic.List[string]
function Say([string]$text) { $lines.Add($text); Write-Host $text }

$agents = @(
    @{ Id = 'kimi-code'; Names = @('kimi'); Pattern = 'kimi'; Homes = @('.kimi-code', '.kimi') },
    @{ Id = 'minimax-code'; Names = @('mcode'); Pattern = 'minimax|mcode|mavis'; Homes = @('.minimax') },
    @{ Id = 'zcode'; Names = @('zcode'); Pattern = 'zcode|z\.ai|zhipu'; Homes = @('.zcode') }
)

$roots = @(
    (Join-Path $env:LOCALAPPDATA 'Programs'),
    $env:LOCALAPPDATA,
    $env:APPDATA,
    $env:ProgramFiles,
    ${env:ProgramFiles(x86)}
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -Unique

$npmPrefix = $null
if (Get-Command npm -ErrorAction SilentlyContinue) {
    $npmPrefix = (& npm prefix -g 2>$null | Select-Object -First 1)
}

Say "orx agent probe — $(Get-Date -Format o)"
Say "Windows: $([Environment]::OSVersion.VersionString); PowerShell $($PSVersionTable.PSVersion)"
Say "node: $((Get-Command node -ErrorAction SilentlyContinue).Source) $(if (Get-Command node -ErrorAction SilentlyContinue) { & node --version })"
Say "git bash: $((Get-Command bash -ErrorAction SilentlyContinue).Source)"
Say "npm global prefix: $npmPrefix"
Say ''

function Find-Candidates($agent) {
    $found = New-Object System.Collections.Generic.List[string]
    foreach ($name in $agent.Names) {
        Get-Command $name -All -ErrorAction SilentlyContinue | ForEach-Object { $found.Add($_.Source) }
        if ($npmPrefix) {
            foreach ($ext in @('.cmd', '.ps1', '')) {
                $p = Join-Path $npmPrefix "$name$ext"
                if (Test-Path $p) { $found.Add($p) }
            }
        }
    }
    foreach ($h in $agent.Homes) {
        $dir = Join-Path $env:USERPROFILE $h
        if (Test-Path $dir) {
            Get-ChildItem $dir -Recurse -Depth 4 -File -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match "^($($agent.Names -join '|'))(\.exe|\.cmd|\.cjs|\.mjs|\.js)?$" } |
                ForEach-Object { $found.Add($_.FullName) }
        }
    }
    foreach ($root in $roots) {
        Get-ChildItem $root -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -match $agent.Pattern } |
            ForEach-Object {
                $found.Add("[app dir] $($_.FullName)")
                Get-ChildItem $_.FullName -Recurse -Depth 6 -File -ErrorAction SilentlyContinue |
                    Where-Object {
                        $_.Name -match "^($($agent.Names -join '|')).*\.(exe|cmd|cjs|mjs|js)$" -or
                        $_.Name -match '^(node|bun)\.exe$' -or
                        $_.Name -match '\.asar$' -or
                        $_.Extension -eq '.exe'
                    } |
                    Select-Object -First 60 |
                    ForEach-Object { $found.Add("  $($_.FullName)  ($($_.Length) bytes)") }
            }
    }
    $found | Select-Object -Unique
}

function Invoke-Capture([string]$exe, [string[]]$argv, [string]$cwd, [int]$timeout) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    if ($exe -match '\.(cmd|bat)$') {
        $psi.FileName = $env:ComSpec
        $psi.Arguments = '/d /s /c ""' + $exe + '" ' + ($argv -join ' ') + '"'
    } else {
        $psi.FileName = $exe
        $psi.Arguments = ($argv | ForEach-Object { if ($_ -match '\s') { '"' + $_ + '"' } else { $_ } }) -join ' '
    }
    $psi.WorkingDirectory = $cwd
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = $true
    $p = [System.Diagnostics.Process]::Start($psi)
    $p.StandardInput.Close()
    $out = $p.StandardOutput.ReadToEndAsync()
    $err = $p.StandardError.ReadToEndAsync()
    if (-not $p.WaitForExit($timeout * 1000)) { try { $p.Kill() } catch {} ; return "TIMEOUT after ${timeout}s`n$($out.Result)`n$($err.Result)" }
    "exit $($p.ExitCode)`n$($out.Result)`n$($err.Result)"
}

# One ACP exchange over stdio: initialize, session/new, and (with -Turn) a
# single session/prompt. Every line the agent writes is recorded verbatim.
function Invoke-Acp([string]$exe, [string]$cwd, [int]$timeout, [bool]$turn) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    if ($exe -match '\.(cmd|bat)$') {
        $psi.FileName = $env:ComSpec
        $psi.Arguments = '/d /s /c ""' + $exe + '" acp"'
    } else {
        $psi.FileName = $exe
        $psi.Arguments = 'acp'
    }
    $psi.WorkingDirectory = $cwd
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.RedirectStandardInput = $true
    $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
    $p = [System.Diagnostics.Process]::Start($psi)
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
                # Reject: the probe never lets an agent act.
                $reply = @{ jsonrpc = '2.0'; id = $m.id; result = @{ outcome = @{ outcome = 'cancelled' } } } | ConvertTo-Json -Depth 10 -Compress
                $log.Add(">> $reply")
                $p.StandardInput.WriteLine($reply)
                continue
            }
            if ($null -ne $m.id -and $m.id -eq $id -and ($null -ne $m.result -or $null -ne $m.error)) { return $m }
        }
        $log.Add('<< TIMEOUT')
        $null
    }

    Send 1 'initialize' @{ protocolVersion = 1; clientCapabilities = @{ fs = @{ readTextFile = $false; writeTextFile = $false }; terminal = $false }; clientInfo = @{ name = 'orx-probe'; version = '0' } }
    $null = Await 1
    Send 2 'session/new' @{ cwd = $cwd; mcpServers = @() }
    $new = Await 2
    if ($turn -and $new -and $new.result.sessionId) {
        Send 3 'session/prompt' @{ sessionId = $new.result.sessionId; prompt = @(@{ type = 'text'; text = 'Reply with the single word OK.' }) }
        $null = Await 3
    }
    try { $p.StandardInput.Close(); if (-not $p.WaitForExit(3000)) { $p.Kill() } } catch {}
    $log.Add("stderr:`n$($errTask.Result)")
    $log -join "`n"
}

$work = Join-Path $env:TEMP ("orx-probe-" + [guid]::NewGuid().ToString('N').Substring(0, 8))
New-Item -ItemType Directory -Path $work | Out-Null
if (Get-Command git -ErrorAction SilentlyContinue) { & git -C $work init -q 2>$null }

foreach ($agent in $agents) {
    Say "=================== $($agent.Id)"
    foreach ($h in $agent.Homes) {
        $dir = Join-Path $env:USERPROFILE $h
        if (Test-Path $dir) {
            Say "config home $dir (top-level entries, names only):"
            Get-ChildItem $dir -Force -ErrorAction SilentlyContinue | ForEach-Object { Say "  $($_.Name)$(if ($_.PSIsContainer) { '\' })" }
        } else {
            Say "config home ${dir}: missing"
        }
    }
    $candidates = @(Find-Candidates $agent)
    Say 'candidates:'
    $candidates | ForEach-Object { Say "  $_" }
    $bin = $candidates | Where-Object { $_ -notmatch '^\s|^\[' -and $_ -match '\.(exe|cmd)$' } | Select-Object -First 1
    if (-not $bin) { Say 'no runnable command found'; Say ''; continue }
    Say "using: $bin"
    Say '--- --version'
    Say (Invoke-Capture $bin @('--version') $work 30)
    Say '--- --help'
    Say (Invoke-Capture $bin @('--help') $work 30)
    if ($agent.Id -eq 'zcode') {
        if ($Turn) {
            Say '--- -p (json)'
            Say (Invoke-Capture $bin @('-p', 'Reply with the single word OK.', '--json', '--mode', 'build', '--no-color') $work $TimeoutSec)
        }
        Say '--- ACP (attempt; zcode may not implement it)'
    } else {
        Say '--- ACP'
    }
    Say (Invoke-Acp $bin $work $TimeoutSec ([bool]$Turn))
    Say ''
}

Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
$lines | Set-Content -Path $report -Encoding UTF8
Write-Host ''
Write-Host "Report written to $report — review it, then share it."
