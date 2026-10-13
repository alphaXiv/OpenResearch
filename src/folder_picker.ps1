$ErrorActionPreference = 'Stop'
# Rust reads stdout as UTF-8, including project paths outside the system code page.
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Windows.Forms

# COM interop for the modern IFileDialog with FOS_PICKFOLDERS, replacing the
# legacy FolderBrowserDialog tree-view picker with the Explorer-style dialog.
$interop = @'
using System;
using System.Runtime.InteropServices;
using System.Windows.Forms;

namespace OpenResearch {
    [ComImport, Guid("DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7")]
    class FileOpenDialogRCW { }

    [ComImport, Guid("42F85136-DB7E-439C-85F1-E4075D135FC8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IFileDialog {
        [PreserveSig] int Show(IntPtr hwndOwner);
        [PreserveSig] int SetFileTypes(uint cFileTypes, IntPtr rgFilterSpec);
        [PreserveSig] int SetFileTypeIndex(uint iFileType);
        [PreserveSig] int GetFileTypeIndex(out uint piFileType);
        [PreserveSig] int Advise(IntPtr pfde, out uint pdwCookie);
        [PreserveSig] int Unadvise(uint dwCookie);
        [PreserveSig] int SetOptions(uint fos);
        [PreserveSig] int GetOptions(out uint pfos);
        [PreserveSig] int SetDefaultFolder(IShellItem psi);
        [PreserveSig] int SetFolder(IShellItem psi);
        [PreserveSig] int GetFolder(out IShellItem ppsi);
        [PreserveSig] int GetCurrentSelection(out IShellItem ppsi);
        [PreserveSig] int SetFileName([MarshalAs(UnmanagedType.LPWStr)] string pszName);
        [PreserveSig] int GetFileName([MarshalAs(UnmanagedType.LPWStr)] out string pszName);
        [PreserveSig] int SetTitle([MarshalAs(UnmanagedType.LPWStr)] string pszTitle);
        [PreserveSig] int SetOkButtonLabel([MarshalAs(UnmanagedType.LPWStr)] string pszText);
        [PreserveSig] int SetFileNameLabel([MarshalAs(UnmanagedType.LPWStr)] string pszLabel);
        [PreserveSig] int GetResult(out IShellItem ppsi);
        [PreserveSig] int AddPlace(IShellItem psi, uint fdap);
        [PreserveSig] int SetDefaultExtension([MarshalAs(UnmanagedType.LPWStr)] string pszDefaultExtension);
        [PreserveSig] int Close(int hr);
        [PreserveSig] int SetClientGuid(ref Guid guid);
        [PreserveSig] int ClearClientData();
        [PreserveSig] int SetFilter(IntPtr pFilter);
    }

    [ComImport, Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IShellItem {
        [PreserveSig] int BindToHandler(IntPtr pbc, ref Guid bhid, ref Guid riid, out IntPtr ppv);
        [PreserveSig] int GetParent(out IShellItem ppsi);
        [PreserveSig] int GetDisplayName(uint sigdnName, [MarshalAs(UnmanagedType.LPWStr)] out string ppszName);
        [PreserveSig] int GetAttributes(uint sfgaoMask, out uint psfgaoAttribs);
        [PreserveSig] int Compare(IShellItem psi, uint hint, out int piOrder);
    }

    public sealed class FolderPicker : IDisposable {
        private const uint FOS_PICKFOLDERS = 0x20;
        private const uint FOS_FORCEFILESYSTEM = 0x40;
        private const uint FOS_PATHMUSTEXIST = 0x800;
        private const uint SIGDN_FILESYSPATH = 0x80058000;

        private IFileDialog _dialog;
        private string _selectedPath;

        public string SelectedPath {
            get { return _selectedPath; }
        }

        public FolderPicker() {
            _dialog = (IFileDialog)new FileOpenDialogRCW();
        }

        public DialogResult ShowDialog(IWin32Window owner) {
            _dialog.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST);
            _dialog.SetTitle("Choose a project folder");

            IntPtr hwnd = owner != null ? owner.Handle : IntPtr.Zero;
            int hr = _dialog.Show(hwnd);

            if (hr == 0) {
                IShellItem item;
                if (_dialog.GetResult(out item) == 0 && item != null) {
                    try {
                        string path;
                        if (item.GetDisplayName(SIGDN_FILESYSPATH, out path) == 0) {
                            _selectedPath = path;
                        }
                    } finally {
                        Marshal.ReleaseComObject(item);
                    }
                }
                return DialogResult.OK;
            }

            // HRESULT_FROM_WIN32(ERROR_CANCELLED) = 0x800704C7
            if ((uint)hr == 0x800704C7) {
                return DialogResult.Cancel;
            }

            throw Marshal.GetExceptionForHR(hr) ?? new COMException("Folder picker failed", hr);
        }

        public void Dispose() {
            if (_dialog != null) {
                Marshal.ReleaseComObject(_dialog);
                _dialog = null;
            }
        }
    }
}
'@

Add-Type -TypeDefinition $interop -ReferencedAssemblies System.Windows.Forms

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

    $dialog = New-Object OpenResearch.FolderPicker
    if ($dialog.ShowDialog($owner) -eq [System.Windows.Forms.DialogResult]::OK) {
        Write-Output $dialog.SelectedPath
    }
} finally {
    if ($null -ne $dialog) { $dialog.Dispose() }
    $owner.Dispose()
}

