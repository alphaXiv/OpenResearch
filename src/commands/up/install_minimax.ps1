$ErrorActionPreference = 'Stop'
# Run MiniMax's official installer. It provisions its own Node.js under
# ~/.minimax-code/runtime, but its final activation step can fail
# ("Staged versioned MCode release validation failed") and leave no mcode.
# In that case install the same npm package with that Node's npm into
# ~/.minimax-code/npm, where OpenResearch also looks for mcode.
$home_ = Join-Path $env:USERPROFILE '.minimax-code'
$lock = Join-Path $home_ '.mcode-update.lock'
if (Test-Path -LiteralPath $lock) {
    Remove-Item -LiteralPath $lock -Recurse -Force -ErrorAction SilentlyContinue
}
try {
    Invoke-RestMethod https://filecdn.minimax.chat/public/install.ps1 | Invoke-Expression
} catch {
    Write-Output "The official MiniMax installer stopped: $($_.Exception.Message)"
}
foreach ($launcher in @('mcode.cmd', 'current\mcode.cmd', 'npm\mcode.cmd')) {
    if (Test-Path -LiteralPath (Join-Path $home_ $launcher)) {
        Write-Output 'MiniMax Code installed successfully.'
        exit 0
    }
}
$node = Get-ChildItem (Join-Path $home_ 'runtime') -Directory -Filter 'node-*' -ErrorAction SilentlyContinue |
    Where-Object { Test-Path (Join-Path $_.FullName 'npm.cmd') } |
    Sort-Object Name -Descending | Select-Object -First 1
if (-not $node) {
    throw 'The MiniMax installer did not provide Node.js, so the npm fallback cannot run. Install Node.js 22.19+ and run: npm install -g @minimax-ai/code'
}
Write-Output "Installing @minimax-ai/code with $($node.FullName)\npm.cmd ..."
$env:PATH = "$($node.FullName);$env:PATH"
if (Test-Path -LiteralPath $lock) {
    Remove-Item -LiteralPath $lock -Recurse -Force -ErrorAction SilentlyContinue
}
& (Join-Path $node.FullName 'npm.cmd') install -g --prefix (Join-Path $home_ 'npm') '@minimax-ai/code@latest' --registry=https://registry.npmjs.org/ --include=optional --foreground-scripts
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
& (Join-Path $home_ 'npm\mcode.cmd') --version
if ($LASTEXITCODE -ne 0) { throw 'MiniMax Code failed to run after installation.' }
Write-Output 'MiniMax Code installed successfully.'
