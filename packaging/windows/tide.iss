#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #error SourceDir is required
#endif
#ifndef OutputDir
  #error OutputDir is required
#endif

[Setup]
AppId={{6690B222-54A9-4FE4-A0D0-1215AA9D3A9E}
AppName=Tide
AppVersion={#AppVersion}
AppPublisher=Tide contributors
AppPublisherURL=https://github.com/ZZray/tide
AppSupportURL=https://github.com/ZZray/tide/issues
AppUpdatesURL=https://github.com/ZZray/tide/releases
DefaultDirName={localappdata}\Programs\Tide
DefaultGroupName=Tide
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=Tide-v{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\assets\brand\tide.ico
UninstallDisplayIcon={app}\tide.exe
LicenseFile=..\..\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
DisableProgramGroupPage=yes
CloseApplications=yes
RestartApplications=no
SetupLogging=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
#if FileExists(AddBackslash(CompilerPath) + "Languages\ChineseSimplified.isl")
Name: "chinesesimplified"; MessagesFile: "compiler:Languages\ChineseSimplified.isl"
#endif

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Tide"; Filename: "{app}\tide.exe"
Name: "{autodesktop}\Tide"; Filename: "{app}\tide.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\tide.exe"; Description: "Launch Tide"; Flags: nowait postinstall skipifsilent
