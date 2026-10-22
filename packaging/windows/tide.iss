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
; In-app updates run silently with /RELAUNCH=window|background and start Tide again.
Filename: "{app}\tide.exe"; Flags: nowait; Check: RelaunchMode('window')
Filename: "{app}\tide.exe"; Parameters: "gui --background"; Flags: nowait; Check: RelaunchMode('background')

[Code]
const
  SYNCHRONIZE = $00100000;

function OpenProcess(DesiredAccess: Cardinal; InheritHandle: Integer; ProcessId: Cardinal): THandle;
  external 'OpenProcess@kernel32.dll stdcall';
function WaitForSingleObject(Handle: THandle; Milliseconds: Cardinal): Cardinal;
  external 'WaitForSingleObject@kernel32.dll stdcall';
function CloseHandle(Handle: THandle): Integer;
  external 'CloseHandle@kernel32.dll stdcall';

{ /WAITPID=<pid>: Tide starts the installer as its last step; wait until that process has
  exited so tide.exe is no longer in use. }
function InitializeSetup(): Boolean;
var
  Pid: Cardinal;
  Handle: THandle;
begin
  Pid := StrToIntDef(ExpandConstant('{param:WAITPID|0}'), 0);
  if Pid <> 0 then
  begin
    Handle := OpenProcess(SYNCHRONIZE, 0, Pid);
    if Handle <> 0 then
    begin
      Log('Waiting for Tide process ' + IntToStr(Pid) + ' to exit');
      WaitForSingleObject(Handle, 60000);
      CloseHandle(Handle);
    end;
  end;
  Result := True;
end;

function RelaunchMode(Mode: String): Boolean;
begin
  Result := ExpandConstant('{param:RELAUNCH|}') = Mode;
end;
