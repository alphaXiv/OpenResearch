; OpenResearch-Setup.exe: a per-user install of the Windows desktop app
; (OpenResearch.exe plus the orx.exe it starts) with a Start menu entry.
;
;   iscc /DAppVersion=<version> /DSourceDir=<dir with both exes> /DOutputDir=<dir> windows\OpenResearch.iss

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
; The app's single-instance mutex (src/commands/app.rs): replacing files under a
; running app would kill it without its quit path, which stops the agents.
AppMutex=Local\OpenResearchApp
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
; Tells Explorer the .orx association below changed.
ChangesAssociations=yes

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\OpenResearch.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\orx.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\OpenResearch"; Filename: "{app}\OpenResearch.exe"; AppUserModelID: "{#AppUserModelID}"
Name: "{autodesktop}\OpenResearch"; Filename: "{app}\OpenResearch.exe"; AppUserModelID: "{#AppUserModelID}"; Tasks: desktopicon

[Registry]
; openresearch:// links (src/commands/app.rs) open the app; the launcher passes them to orx.exe.
Root: HKCU; Subkey: "Software\Classes\openresearch"; ValueType: string; ValueName: ""; ValueData: "URL:OpenResearch"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\openresearch"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\openresearch\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\OpenResearch.exe"",0"
Root: HKCU; Subkey: "Software\Classes\openresearch\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\OpenResearch.exe"" ""%1"""
; Shared projects (src/local/share.rs) open in the app, which imports them.
Root: HKCU; Subkey: "Software\Classes\.orx"; ValueType: string; ValueName: ""; ValueData: "OpenResearch.Project"; Flags: uninsdeletevalue uninsdeletekeyifempty
Root: HKCU; Subkey: "Software\Classes\OpenResearch.Project"; ValueType: string; ValueName: ""; ValueData: "OpenResearch project"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\OpenResearch.Project\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\OpenResearch.exe"",0"
Root: HKCU; Subkey: "Software\Classes\OpenResearch.Project\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\OpenResearch.exe"" ""%1"""

[Run]
Filename: "{app}\OpenResearch.exe"; Description: "{cm:LaunchProgram,OpenResearch}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
; WebView2's profile, and what orx.exe's self-update leaves beside itself.
Type: filesandordirs; Name: "{app}\orx.exe.WebView2"
Type: files; Name: "{app}\orx.exe.*.old"
Type: filesandordirs; Name: "{app}\.orx-update-*"
Type: files; Name: "{app}\download-click"

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

procedure ReportMissingWebView2;
begin
  SuppressibleMsgBox('OpenResearch needs the Microsoft Edge WebView2 Runtime, which could not be ' +
    'installed. Install it from https://developer.microsoft.com/microsoft-edge/webview2/ ' +
    'and then start OpenResearch.', mbError, MB_OK, IDOK);
end;

// The browser records the page a download started from in the installer's
// Zone.Identifier stream; installer_click_id in src/telemetry.rs reads the id saved here.
procedure SaveDownloadClick;
var
  Zone: AnsiString;
  Query: String;
  At: Integer;
begin
  try
    if not LoadStringFromFile(ExpandConstant('{srcexe}') + ':Zone.Identifier', Zone) then
      Exit;
    Query := String(Zone);
    At := Pos('ReferrerUrl=https://openresearch.sh/download?', Query);
    if At = 0 then
      Exit;
    Delete(Query, 1, At + Length('ReferrerUrl=https://openresearch.sh/download'));
    At := Pos(#13, Query);
    if At = 0 then
      At := Pos(#10, Query);
    if At > 0 then
      Query := Copy(Query, 1, At - 1);
    Query := '&' + Query;
    At := Pos('&c=', Query);
    if At = 0 then
      Exit;
    Delete(Query, 1, At + 2);
    At := Pos('&', Query);
    if At > 0 then
      Query := Copy(Query, 1, At - 1);
    SaveStringToFile(ExpandConstant('{app}\download-click'), Query, False);
  except
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep <> ssPostInstall then
    Exit;
  SaveDownloadClick;
  if WebView2Installed then
    Exit;
  // The bootstrapper runs hidden and can take minutes.
  WizardForm.StatusLabel.Caption := 'Installing the Microsoft Edge WebView2 Runtime...';
  try
    DownloadTemporaryFile(WebView2Bootstrapper, 'MicrosoftEdgeWebview2Setup.exe', '', nil);
    if not Exec(ExpandConstant('{tmp}\MicrosoftEdgeWebview2Setup.exe'), '/silent /install', '',
      SW_HIDE, ewWaitUntilTerminated, ResultCode) or not WebView2Installed then
      ReportMissingWebView2;
  except
    ReportMissingWebView2;
  end;
end;

// Without the runtime the app can't open its window, so don't offer to start it.
procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpFinished) and not WebView2Installed
    and (WizardForm.RunList.Items.Count > 0) then
  begin
    WizardForm.RunList.Checked[0] := False;
    WizardForm.RunList.Visible := False;
  end;
end;
