$ErrorActionPreference = 'Stop'
# Windows PowerShell's progress bar slows Invoke-WebRequest to a crawl.
$ProgressPreference = 'SilentlyContinue'
# Windows PowerShell 5.1 can default to TLS 1.0, which GitHub and npm refuse.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
$arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
# ponytail: baseline supports all x64 CPUs; select AVX2 builds if performance requires it.
$asset = switch ($arch) {
    'X64' { 'x64-baseline' }
    'Arm64' { 'arm64' }
    default { throw "OpenCode has no Windows build for $arch processors." }
}
$temp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.IO.Path]::GetRandomFileName())
$destination = Join-Path $env:USERPROFILE '.opencode\bin'
$installed = Join-Path $destination 'opencode.exe'
$release = "https://github.com/anomalyco/opencode/releases/latest/download/opencode-windows-$asset.zip"
New-Item -ItemType Directory -Path $temp -Force | Out-Null
try {
    Write-Output 'Downloading OpenCode...'
    $archive = Join-Path $temp 'opencode.zip'
    $binary = $null
    # Unlike curl.exe, Invoke-WebRequest uses the system proxy and tolerates
    # revocation servers a filtered network cannot reach.
    try {
        Invoke-WebRequest -UseBasicParsing -Uri $release -OutFile $archive -TimeoutSec 30
        Expand-Archive -Path $archive -DestinationPath $temp -Force
        $binary = Join-Path $temp 'opencode.exe'
    } catch {
        Write-Output "GitHub download failed: $($_.Exception.Message)"
    }
    # The same build is published to npm, which networks that block GitHub's downloads often allow.
    if (-not $binary) {
        try {
            $package = Invoke-RestMethod -UseBasicParsing -Uri "https://registry.npmjs.org/opencode-windows-$asset/latest" -TimeoutSec 30
            $tarball = Join-Path $temp 'opencode.tgz'
            Invoke-WebRequest -UseBasicParsing -Uri $package.dist.tarball -OutFile $tarball -TimeoutSec 30
            $expected = -join ([Convert]::FromBase64String(($package.dist.integrity -replace '^sha512-', '')) | ForEach-Object { $_.ToString('x2') })
            if ((Get-FileHash -LiteralPath $tarball -Algorithm SHA512).Hash -ne $expected) { throw 'npm package checksum mismatch.' }
            & (Join-Path $env:SystemRoot 'System32\tar.exe') -xzf $tarball -C $temp
            if ($LASTEXITCODE -ne 0) { throw "tar exited with $LASTEXITCODE." }
            $binary = Join-Path $temp 'package\bin\opencode.exe'
        } catch {
            Write-Output "npm download failed: $($_.Exception.Message)"
        }
    }
    if (-not $binary) {
        curl.exe -fL --retry 2 --connect-timeout 15 --max-time 300 $release -o $archive
        # Keep curl's own exit code so distinct network faults stay apart; the
        # throws below surface as exit 1 with their message.
        if ($LASTEXITCODE -ne 0) {
            # Straight to stderr: an error record raised here prints this whole script as its source.
            [Console]::Error.WriteLine("OpenCode download failed (curl exit $LASTEXITCODE).")
            exit $LASTEXITCODE
        }
        Expand-Archive -Path $archive -DestinationPath $temp -Force
        $binary = Join-Path $temp 'opencode.exe'
    }
    & $binary --version
    if ($LASTEXITCODE -ne 0) { throw 'OpenCode failed to run after extraction.' }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    # Windows won't overwrite a running exe, but renaming it aside succeeds
    # while it is open; killing the process could be the user's own session.
    $displaced = $null
    if (Test-Path -LiteralPath $installed) {
        $displaced = "$installed.old-$([System.IO.Path]::GetRandomFileName())"
        try {
            Move-Item -LiteralPath $installed -Destination $displaced -Force
        } catch {
            $displaced = $null
            # A lock is the usual cause, but a denial or AV hold lands here too,
            # so the OS message goes out rather than a guess at which one it was.
            throw "OpenCode's program file cannot be replaced: $($_.Exception.Message) If OpenCode is running (including any ""opencode serve"" process), close it and run this install again."
        }
    }
    try {
        Move-Item -Path $binary -Destination $installed -Force
    } catch {
        # Put the working install back rather than leaving no opencode at all.
        if ($displaced) { Move-Item -LiteralPath $displaced -Destination $installed -Force }
        throw
    }
    # Only this install's own displaced copy: a sweep would delete the rollback
    # copy a concurrent install still needs. Still locked means still open.
    if ($displaced -and (Test-Path -LiteralPath $displaced)) {
        Remove-Item -LiteralPath $displaced -Force -ErrorAction SilentlyContinue
        if (Test-Path -LiteralPath $displaced) {
            Write-Output "The previous program file is still in use and was left at $displaced. Delete it once OpenCode is closed."
        }
    }
} finally {
    Remove-Item -LiteralPath $temp -Recurse -Force -ErrorAction SilentlyContinue
}
Write-Output 'OpenCode installed successfully.'
