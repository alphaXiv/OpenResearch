$ErrorActionPreference = 'Stop'
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
# ponytail: baseline supports all x64 CPUs; select AVX2 builds if performance requires it.
$asset = switch ($arch) {
    'X64' { 'x64-baseline' }
    'Arm64' { 'arm64' }
    default { throw "Unsupported architecture: $arch" }
}
$temp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
$destination = Join-Path $env:USERPROFILE '.opencode\bin'
New-Item -ItemType Directory -Path $temp -Force | Out-Null
try {
    Write-Output 'Downloading OpenCode...'
    $archive = Join-Path $temp 'opencode.zip'
    curl.exe -fL --retry 2 --connect-timeout 15 --max-time 300 "https://github.com/anomalyco/opencode/releases/latest/download/opencode-windows-$asset.zip" -o $archive
    if ($LASTEXITCODE -ne 0) { throw "OpenCode download failed (curl exit $LASTEXITCODE)." }
    Expand-Archive -Path $archive -DestinationPath $temp
    $binary = Join-Path $temp 'opencode.exe'
    & $binary --version
    if ($LASTEXITCODE -ne 0) { throw 'OpenCode failed to run after extraction.' }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    Move-Item -Path $binary -Destination (Join-Path $destination 'opencode.exe') -Force
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output 'OpenCode installed successfully.'
