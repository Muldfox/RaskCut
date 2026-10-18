; RaskCut installer (Inno Setup). Built by GitHub Actions, see .github/workflows/release.yml
; Expects the finished program (RaskCut.exe + ffmpeg files) in ..\dist

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{7EEC1295-F7BA-4848-966C-A4322ACDCFD6}
AppName=RaskCut
AppVersion={#AppVersion}
AppVerName=RaskCut {#AppVersion}
DefaultDirName={localappdata}\Programs\RaskCut
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
OutputDir=Output
OutputBaseFilename=RaskCut-Setup-{#AppVersion}
Compression=lzma2/ultra64
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\RaskCut.exe
CloseApplications=yes
WizardStyle=modern

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "..\dist\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\RaskCut"; Filename: "{app}\RaskCut.exe"
Name: "{autodesktop}\RaskCut"; Filename: "{app}\RaskCut.exe"; Tasks: desktopicon

[Run]
; Also runs after a silent in-app update, so RaskCut restarts by itself.
Filename: "{app}\RaskCut.exe"; Description: "Start RaskCut"; Flags: nowait postinstall
