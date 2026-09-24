; OpenResearch-Setup.exe: a per-user install of the Windows desktop app
; (OpenResearch.exe plus the orx.exe it starts) with a Start menu entry.
;
;   iscc /DAppVersion=<version> /DSourceDir=<dir with both exes> /DOutputDir=<dir> windows\OpenResearch.iss
;
; See docs/windows.md.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<directory holding OpenResearch.exe and orx.exe>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

; Must match set_taskbar_identity in src/commands/app.rs.
#define AppUserModelID "alphaXiv.OpenResearch"

[Setup]
; Never change: Windows tells installs of the same app apart by this id.
AppId={{E9099BE3-5087-4A2A-9296-4DC836929049}
AppName=OpenResearch
AppVersion={#AppVersion}
AppPublisher=alphaXiv
AppPublisherURL=https://openresearch.sh
; Per-user and writable, so orx.exe can update itself in place without admin.
DefaultDirName={localappdata}\Programs\OpenResearch
DisableDirPage=yes
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=OpenResearch-Setup
SetupIconFile=OpenResearch.ico
UninstallDisplayIcon={app}\OpenResearch.exe
UninstallDisplayName=OpenResearch
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; The launcher starts the app; Restart Manager restarting orx.exe would not.
RestartApplications=no

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\OpenResearch.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\orx.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\OpenResearch"; Filename: "{app}\OpenResearch.exe"; AppUserModelID: "{#AppUserModelID}"
Name: "{autodesktop}\OpenResearch"; Filename: "{app}\OpenResearch.exe"; AppUserModelID: "{#AppUserModelID}"; Tasks: desktopicon

[Run]
Filename: "{app}\OpenResearch.exe"; Description: "{cm:LaunchProgram,OpenResearch}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; WebView2's profile, and what orx.exe's self-update leaves beside itself.
Type: filesandordirs; Name: "{app}\orx.exe.WebView2"
Type: files; Name: "{app}\orx.exe.*.old"
Type: filesandordirs; Name: "{app}\.orx-update-*"

[Code]
const
  WebView2ClientKey = 'Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}';
  WebView2Bootstrapper = 'https://go.microsoft.com/fwlink/p/?LinkId=2124703';

function HasWebView2Version(RootKey: Integer; SubKey: String): Boolean;
var
  Version: String;
begin
  Result := RegQueryStringValue(RootKey, SubKey, 'pv', Version)
    and (Version <> '') and (Version <> '0.0.0.0');
end;

// Microsoft's documented check: a machine-wide or a per-user runtime.
function WebView2Installed: Boolean;
begin
  Result := HasWebView2Version(HKLM, 'SOFTWARE\WOW6432Node\' + WebView2ClientKey)
    or HasWebView2Version(HKCU, 'Software\' + WebView2ClientKey);
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if (CurStep <> ssPostInstall) or WebView2Installed then
    Exit;
  try
    DownloadTemporaryFile(WebView2Bootstrapper, 'MicrosoftEdgeWebview2Setup.exe', '', nil);
    Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe'), '/silent /install', '',
      SW_HIDE, ewWaitUntilTerminated, ResultCode);
  except
    SuppressibleMsgBox('OpenResearch needs the Microsoft Edge WebView2 Runtime, which could not be ' +
      'downloaded. Install it from https://developer.microsoft.com/microsoft-edge/webview2/ ' +
      'and then start OpenResearch.', mbError, MB_OK, IDOK);
  end;
end;
