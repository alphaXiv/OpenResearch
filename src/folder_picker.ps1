$ErrorActionPreference = 'Stop'
# Rust reads stdout as UTF-8, including project paths outside the system code page.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Windows.Forms

$owner = New-Object System.Windows.Forms.Form
$dialog = $null
try {
    # An ownerless dialog launched by the dashboard can open behind the browser.
    # Show a transparent, topmost owner so its modal picker is brought forward.
    $owner.ShowInTaskbar = $false
    $owner.Opacity = 0
    $owner.TopMost = $true
    $owner.StartPosition = [System.Windows.Forms.FormStartPosition]::CenterScreen
    $owner.Show()
    $owner.Activate()

    $dialog = New-Object System.Windows.Forms.FolderBrowserDialog
    $dialog.Description = 'Choose a project folder'
    if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) {
        Write-Output $dialog.SelectedPath
    }
} finally {
    if ($null -ne $dialog) { $dialog.Dispose() }
    $owner.Dispose()
}
