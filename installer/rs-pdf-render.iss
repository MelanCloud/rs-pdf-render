; The Windows installer. Build it with Inno Setup 6, after a release build and
; with PDFium unpacked in pdfium-win-x64 (see the README):
;
;   cargo build --release
;   ISCC installer\rs-pdf-render.iss
;
; It installs for the current user by default (no admin prompt), registers the
; app for PDFs so it shows in "Open with" and Settings > Default apps, and
; removes all of that again on uninstall. Windows doesn't let installers make
; themselves the default; the last page offers to open the Settings page for it.

#define AppName "rs-pdf-render"
#define AppExe "rs-pdf-render.exe"
#define AppVersion GetStringFileInfo("..\target\release\" + AppExe, "ProductVersion")
#define ProgId "rs-pdf-render.pdf"
#define Pdfium "..\pdfium-win-x64"

[Setup]
AppId={{7430C6D2-0747-4E63-8BCF-4435748AE9BC}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=MelanCloud
AppPublisherURL=https://github.com/MelanCloud/rs-pdf-render
DefaultDirName={autopf}\{#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
ChangesAssociations=yes
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir=..\target\installer
OutputBaseFilename={#AppName}-{#AppVersion}-setup

[Tasks]
Name: desktopicon; Description: "Create a desktop shortcut"; Flags: unchecked

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#Pdfium}\bin\pdfium.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"
; PDFium's license and those of the libraries built into it.
Source: "{#Pdfium}\LICENSE"; DestDir: "{app}\licenses"; DestName: "pdfium-binaries.txt"
Source: "{#Pdfium}\licenses\*"; DestDir: "{app}\licenses"

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; A file type for PDFs that opens them in the app.
Root: HKA; Subkey: "Software\Classes\{#ProgId}"; ValueType: string; ValueData: "PDF Document"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\{#ProgId}\DefaultIcon"; ValueType: string; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\Classes\{#ProgId}\shell\open\command"; ValueType: string; ValueData: """{app}\{#AppExe}"" ""%1"""
; Offered under "Open with" for PDFs.
Root: HKA; Subkey: "Software\Classes\.pdf\OpenWithProgids"; ValueType: string; ValueName: "{#ProgId}"; ValueData: ""; Flags: uninsdeletevalue
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\Classes\Applications\{#AppExe}\SupportedTypes"; ValueType: string; ValueName: ".pdf"; ValueData: ""
; Listed in Settings > Apps > Default apps.
Root: HKA; Subkey: "Software\{#AppName}"; Flags: uninsdeletekey
Root: HKA; Subkey: "Software\{#AppName}\Capabilities"; ValueType: string; ValueName: "ApplicationName"; ValueData: "{#AppName}"
Root: HKA; Subkey: "Software\{#AppName}\Capabilities"; ValueType: string; ValueName: "ApplicationDescription"; ValueData: "A small, fast PDF reader."
Root: HKA; Subkey: "Software\{#AppName}\Capabilities"; ValueType: string; ValueName: "ApplicationIcon"; ValueData: "{app}\{#AppExe},0"
Root: HKA; Subkey: "Software\{#AppName}\Capabilities\FileAssociations"; ValueType: string; ValueName: ".pdf"; ValueData: "{#ProgId}"
Root: HKA; Subkey: "Software\RegisteredApplications"; ValueType: string; ValueName: "{#AppName}"; ValueData: "Software\{#AppName}\Capabilities"; Flags: uninsdeletevalue

[Run]
Filename: "{app}\{#AppExe}"; Description: "Open {#AppName}"; Flags: nowait postinstall skipifsilent
Filename: "ms-settings:defaultapps?registeredAppUser={#AppName}"; Description: "Choose it as the default PDF app"; Flags: shellexec nowait postinstall skipifsilent unchecked; Check: not IsAdminInstallMode
Filename: "ms-settings:defaultapps?registeredAppMachine={#AppName}"; Description: "Choose it as the default PDF app"; Flags: shellexec nowait postinstall skipifsilent unchecked; Check: IsAdminInstallMode
