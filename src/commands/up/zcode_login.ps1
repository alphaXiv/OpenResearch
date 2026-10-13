$LASTEXITCODE = 0
$prompted = $false
$opened = $false
& <COMMAND> 2>&1 | ForEach-Object {
    $line = "$_"
    $line
    if ($line -match 'Open this URL') { $prompted = $true }
    if ($prompted -and -not $opened -and $line -match 'https://[^\s\x1b]+') {
        $opened = $true
        try { Start-Process -FilePath $Matches[0] } catch { }
    }
}
exit $LASTEXITCODE
