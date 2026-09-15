# Keep the real WinForms owner, replacing only the dialog that requires user input.
function New-Object {
    param([string]$TypeName)
    if ($TypeName -ne 'System.Windows.Forms.FolderBrowserDialog') {
        return Microsoft.PowerShell.Utility\New-Object -TypeName $TypeName
    }
    $picker = [PSCustomObject]@{ Description = ''; SelectedPath = $env:ORX_PICKER_TEST_PATH }
    $picker | Add-Member ScriptMethod ShowDialog {
        param($owner)
        if ($null -eq $owner -or !$owner.Visible -or !$owner.TopMost -or !$owner.IsHandleCreated) {
            throw 'Folder picker needs a visible, topmost owner with a window handle'
        }
        $global:pickerTestOwner = $owner
        if ($env:ORX_PICKER_TEST_RESULT -eq 'error') { throw 'Simulated folder picker failure' }
        if ($env:ORX_PICKER_TEST_RESULT -eq 'cancel') { return [System.Windows.Forms.DialogResult]::Cancel }
        return [System.Windows.Forms.DialogResult]::OK
    }
    $picker | Add-Member ScriptMethod Dispose { $global:pickerTestDisposed = $true }
    return $picker
}

function Assert-PickerDisposed {
    if (!$global:pickerTestDisposed -or !$global:pickerTestOwner.IsDisposed) {
        throw 'Picker resources were not disposed'
    }
}
