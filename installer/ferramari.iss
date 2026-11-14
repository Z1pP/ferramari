; Inno Setup 6 script for Ferramari on Windows.
;
; Build through installer\build.ps1, which stages both binaries and the locale
; catalogs and passes AppVersion/SourceDir/LocaleDir/OutputDir on the command
; line. The defaults below make a direct ISCC run from this directory work when
; the release build already exists in ..\target\release.

#ifndef AppVersion
  #define AppVersion "2.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef LocaleDir
  #define LocaleDir "..\target\release\locale"
#endif
#ifndef OutputDir
  #define OutputDir "Output"
#endif

[Setup]
; A stable AppId keeps upgrades in the same installation directory. Changing it
; would make Windows treat a new build as a separate application.
AppId={{7C1E0F4A-9B2D-4F6A-9E3B-2A5D7C8E1F40}
AppName=Ferramari
AppVersion={#AppVersion}
AppVerName=Ferramari {#AppVersion}
AppPublisher=Ferramari contributors
AppPublisherURL=https://github.com/Z1pP/ferramari
AppSupportURL=https://github.com/Z1pP/ferramari/issues
AppUpdatesURL=https://github.com/Z1pP/ferramari/releases
DefaultDirName={autopf}\Ferramari
DefaultGroupName=Ferramari
DisableProgramGroupPage=yes
; Per-user by default, so no UAC prompt is needed. An administrator can still
; choose an all-users install from the elevation dialog.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
OutputDir={#OutputDir}
OutputBaseFilename=Ferramari-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayName=Ferramari {#AppVersion}
UninstallDisplayIcon={app}\ferramari.exe
CloseApplications=yes
UsePreviousAppDir=yes
SetupLogging=yes
; There is no .ico asset in the repository yet; the default setup icon is
; honest and does not pretend to be the Linux SVG.
VersionInfoVersion={#AppVersion}
VersionInfoDescription=Ferramari video wallpaper
VersionInfoProductName=Ferramari
VersionInfoProductVersion={#AppVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\ferramari.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\ferramari-ui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
; build.ps1 stages catalogs under <bin>\locale, which is also the portable
; layout that paths::locale_dir() checks first.
Source: "{#LocaleDir}\*"; DestDir: "{app}\locale"; Flags: ignoreversion recursesubdirs createallsubdirs skipifsourcedoesntexist

[Icons]
Name: "{group}\Ferramari"; Filename: "{app}\ferramari.exe"
Name: "{group}\Uninstall Ferramari"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Ferramari"; Filename: "{app}\ferramari.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ferramari.exe"; Description: "{cm:LaunchProgram,Ferramari}"; Flags: nowait postinstall skipifsilent

; Deliberately no [UninstallDelete] for user data. Config, library, signing
; keys, packages and the recorded original wallpaper live in the user profile
; and must survive an uninstall unless the user removes them explicitly.
