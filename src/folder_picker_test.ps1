# Keep the real WinForms owner, replacing only the dialog that requires user input.
function New-Object {
    param([string]$TypeName)
    if ($TypeName -ne 'OpenResearch.FolderPicker') {
        return Microsoft.PowerShell.Utility\New-Object -TypeName $TypeName
    }
    $picker = [PSCustomObject]@{ SelectedPath = $env:ORX_PICKER_TEST_PATH }
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

function Test-FolderPickerSmoke {
    # Instantiate the real underlying OpenResearch.FolderPicker COM object directly.
    $picker = Microsoft.PowerShell.Utility\New-Object -TypeName 'OpenResearch.FolderPicker'
    try {
        $expected = [OpenResearch.FolderPicker]::FolderPickerOptions
        $initial = $picker.GetOptions()
        if (($initial -band $expected) -ne $expected) {
            throw "Initial options 0x{0:X} missing expected flags 0x{1:X}" -f $initial, $expected
        }
        $custom = $expected -bor 0x1000 # FOS_FILEMUSTEXIST
        $picker.SetOptions($custom)
        $updated = $picker.GetOptions()
        if (($updated -band $custom) -ne $custom) {
            throw "Updated options 0x{0:X} missing flags 0x{1:X}" -f $updated, $custom
        }
    } finally {
        $picker.Dispose()
    }
}

