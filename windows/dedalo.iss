; Instalador de dedalo para Windows (Inno Setup 6).
; Lo compila .github/workflows/release.yml:  iscc /DAppVersion=0.2.0 windows\dedalo.iss
; Necesita target\release\dedalo.exe ya compilado.
;
; Se instala para el usuario (sin pedir permisos de administrador), en
; %LOCALAPPDATA%\Programs\dedalo.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
; Identificador fijo: con él Windows reconoce las actualizaciones y el desinstalador.
AppId={{5869AF55-BB9F-43C8-9D8D-035AD537B87C}
AppName=dedalo
AppVersion={#AppVersion}
AppVerName=dedalo {#AppVersion}
AppPublisher=airylp
AppPublisherURL=https://github.com/airylp/dedalo
AppSupportURL=https://github.com/airylp/dedalo/issues
AppUpdatesURL=https://github.com/airylp/dedalo/releases
DefaultDirName={autopf}\dedalo
DefaultGroupName=dedalo
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\target\instalador
OutputBaseFilename=dedalo-{#AppVersion}-setup
SetupIconFile=dedalo.ico
UninstallDisplayIcon={app}\dedalo.exe
UninstallDisplayName=dedalo
LicenseFile=..\LICENSE
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; Si dedalo está abierto, el instalador ofrece cerrarlo (Restart Manager).
CloseApplications=yes
RestartApplications=no
ShowLanguageDialog=auto

[Languages]
Name: "es"; MessagesFile: "compiler:Languages\Spanish.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\dedalo.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
; OpenGL por software (Mesa llvmpipe) para equipos sin tarjeta gráfica utilizable.
Source: "..\target\release\opengl32.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\libgallium_wgl.dll"; DestDir: "{app}"; Flags: ignoreversion
Source: "MESA.txt"; DestDir: "{app}"; Flags: ignoreversion

[InstallDelete]
; Hasta la 0.1.3 la app se llamaba «dedalo».
Type: files; Name: "{app}\dedalo.exe"
Type: files; Name: "{autoprograms}\dedalo.lnk"
Type: files; Name: "{autodesktop}\dedalo.lnk"

[Icons]
Name: "{autoprograms}\dedalo"; Filename: "{app}\dedalo.exe"
Name: "{autodesktop}\dedalo"; Filename: "{app}\dedalo.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\dedalo.exe"; Description: "{cm:LaunchProgram,dedalo}"; Flags: nowait postinstall skipifsilent

[Code]
// Guarda el idioma elegido en el instalador como idioma de la app (si aún no
// hay uno elegido), igual que install.sh en macOS/Linux: `idioma = "xx"` al
// principio de sesion.toml.
procedure CurStepChanged(CurStep: TSetupStep);
var
  Dir, F, Rest: String;
  Lines: TArrayOfString;
  I: Integer;
  Has: Boolean;
begin
  if CurStep <> ssPostInstall then
    Exit;
  Dir := ExpandConstant('{userappdata}\dedalo');
  F := Dir + '\sesion.toml';
  Has := False;
  Rest := '';
  if LoadStringsFromFile(F, Lines) then
    for I := 0 to GetArrayLength(Lines) - 1 do
    begin
      if Pos('idioma', Trim(Lines[I])) = 1 then
        Has := True;
      Rest := Rest + Lines[I] + #13#10;
    end;
  if not Has then
  begin
    ForceDirectories(Dir);
    SaveStringToFile(F, 'idioma = "' + ActiveLanguage + '"' + #13#10 + Rest, False);
  end;
end;
