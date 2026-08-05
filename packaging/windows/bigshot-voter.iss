; Inno Setup script for Big-Shot Voting
;
; Default install is per-user under %LOCALAPPDATA%\Programs\bigshot-voter
; so the process can write next to the executable. That keeps the app's
; default data directory ({exe_dir}\data) working without elevation or a
; separate data-dir override.
;
; Build (from repo root, after a release binary exists):
;   iscc packaging\windows\bigshot-voter.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#ifndef SourceExe
  #define SourceExe "..\..\target\release\bigshot-voter.exe"
#endif

#ifndef OutputDir
  #define OutputDir "..\..\target\windows-installer"
#endif

#define MyAppName "Big-Shot Voting"
#define MyAppPublisher "Big-Shot Voting"
#define MyAppExeName "bigshot-voter.exe"
#define MyAppURL "http://127.0.0.1:17843"
#define MyAppId "bigshot-voter"

[Setup]
AppId={{A7C3E9B1-4F2D-4A8E-9C1B-6D5E8F0A2B3C}
AppName={#MyAppName}
AppVersion={#AppVersion}
AppVerName={#MyAppName} {#AppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={localappdata}\Programs\bigshot-voter
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Per-user install: no UAC, and the install tree is writable so data/ works.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutputDir}
OutputBaseFilename=BigShotVoting-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\{#MyAppExeName}
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Do not touch user data on uninstall (database, uploads, secret).
CloseApplications=yes
RestartApplications=no
InfoAfterFile=
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked
Name: "startmenu"; Description: "Create a Start Menu shortcut"; GroupDescription: "{cm:AdditionalIcons}"; Flags: checkedonce

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "{#MyAppExeName}"; Flags: ignoreversion
Source: "README-install.txt"; DestDir: "{app}"; DestName: "README.txt"; Flags: ignoreversion isreadme

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Comment: "Start the voting server"; Tasks: startmenu
Name: "{group}\Uninstall {#MyAppName}"; Filename: "{uninstallexe}"; Tasks: startmenu
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "Start {#MyAppName} now"; Flags: nowait postinstall skipifsilent unchecked

[UninstallDelete]
; Only remove empty leftovers under the install dir — never wipe data/.
Type: dirifempty; Name: "{app}"

[Code]
function InitializeUninstall(): Boolean;
begin
  Result := True;
  // Leave {app}\data alone intentionally. Config lives in %APPDATA%\bigshot-voter.
  MsgBox(
    'Uninstall removes the program files only.'#13#10#13#10 +
    'Your voting data (database, uploads, secret key) in the install folder''s ' +
    'data\ directory, and settings in %APPDATA%\bigshot-voter\, are kept.',
    mbInformation, MB_OK);
end;
