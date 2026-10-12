; Installer di Wheelor (Inno Setup 6.6 o successivo).
; Non compilarlo direttamente: usa installer\build.ps1, che compila Wheelor in release e passa
; la versione letta da Cargo.toml (ISCC /DAppVersion=x.y.z installer\wheelor.iss).
;
; Installazione per l'utente corrente in %LOCALAPPDATA%\Programs\Wheelor: niente permessi
; di amministratore, aggiornamenti in place (stesso AppId), disinstallazione da Impostazioni > App.
; Opzioni da riga di comando utili (es. per winget): /SILENT, /VERYSILENT, /TASKS="autostart".

#ifndef AppVersion
  #error Compila con installer\build.ps1 (oppure passa /DAppVersion=x.y.z a ISCC)
#endif

#define AppName "Wheelor"
#define AppExe "wheelor.exe"
#define AppPublisher "Wheelor"
; Identifica l'app per aggiornamenti e disinstallazione: non va mai cambiato.
#define AppGuid "A65392FC-AA5F-46D3-85A3-EBDAAD9B033C"
#define SettingsTitle "Wheelor – Impostazioni"

[Setup]
AppId={{{#AppGuid}}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
; Da compilare quando ci sarà una pagina pubblica del progetto:
; AppPublisherURL=https://…
; AppSupportURL=https://…/issues
; AppUpdatesURL=https://…/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
VersionInfoDescription={#AppName} Setup

PrivilegesRequired=lowest
DefaultDirName={autopf}\{#AppName}
DisableDirPage=yes
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 1809 o successivo.
MinVersion=10.0.17763

OutputDir=..\dist
OutputBaseFilename=Wheelor-Setup-{#AppVersion}
SetupIconFile=..\assets\wheelor.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
Compression=lzma2/ultra64
SolidCompression=yes

; Aspetto moderno, chiaro o scuro come Windows; logo con trasparenza (vedi tools/make-icon).
WizardStyle=modern dynamic
WizardSmallImageFile=..\assets\installer\wizard-55.bmp,..\assets\installer\wizard-64.bmp,..\assets\installer\wizard-83.bmp,..\assets\installer\wizard-92.bmp,..\assets\installer\wizard-110.bmp,..\assets\installer\wizard-119.bmp,..\assets\installer\wizard-138.bmp
; Nel tema scuro Inno Setup userebbe le sue immagini: il nostro logo va bene su entrambi.
WizardSmallImageFileDynamicDark=..\assets\installer\wizard-55.bmp,..\assets\installer\wizard-64.bmp,..\assets\installer\wizard-83.bmp,..\assets\installer\wizard-92.bmp,..\assets\installer\wizard-110.bmp,..\assets\installer\wizard-119.bmp,..\assets\installer\wizard-138.bmp
; Pannello a sinistra della pagina finale.
WizardImageFile=..\assets\installer\panel-164.bmp,..\assets\installer\panel-192.bmp,..\assets\installer\panel-246.bmp,..\assets\installer\panel-273.bmp,..\assets\installer\panel-328.bmp,..\assets\installer\panel-355.bmp,..\assets\installer\panel-410.bmp
WizardImageFileDynamicDark=..\assets\installer\panel-164.bmp,..\assets\installer\panel-192.bmp,..\assets\installer\panel-246.bmp,..\assets\installer\panel-273.bmp,..\assets\installer\panel-328.bmp,..\assets\installer\panel-355.bmp,..\assets\installer\panel-410.bmp
WizardImageAlphaFormat=premultiplied

; Wheelor in background viene chiuso dal codice qui sotto; se è aperta la finestra delle
; impostazioni, ci pensa la Gestione riavvio di Windows.
CloseApplications=yes
RestartApplications=no
SetupMutex=Wheelor.Setup
ShowLanguageDialog=auto
LanguageDetectionMethod=uilanguage

; Firma del codice (evita l'avviso di SmartScreen): definire un SignTool con il certificato, es.
;   SignTool=firma $f   e in ISCC /Sfirma="signtool.exe sign /fd sha256 /tr http://… /td sha256 $p"
; e firmare anche target\release\wheelor.exe prima di compilare l'installer.

[Languages]
Name: "it"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
it.AutoStart=Avvia Wheelor all'accesso a Windows
en.AutoStart=Start Wheelor when I sign in to Windows
it.ShortcutComment=Azioni rapide su una ruota attorno al cursore
en.ShortcutComment=Quick actions on a wheel around the cursor
it.DeleteSettings=Vuoi eliminare anche le impostazioni e le azioni salvate?%n%nSe pensi di reinstallare Wheelor, scegli No.
en.DeleteSettings=Do you also want to delete your settings and saved actions?%n%nIf you plan to reinstall Wheelor, choose No.

[Tasks]
Name: "autostart"; Description: "{cm:AutoStart}"

[Files]
Source: "..\target\release\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"; Comment: "{cm:ShortcutComment}"

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[Code]
const
  RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{{#AppGuid}}_is1';
  MainClass = 'WheelorMain';
  WM_CLOSE = $0010;

var
  WasRunning: Boolean;

{ Chiede alla finestra di chiudersi e aspetta che sparisca (al massimo Tenths decimi di secondo). }
function CloseWindow(Wnd: HWND; ByClass: Boolean; Name: String; Tenths: Integer): Boolean;
var
  I: Integer;
begin
  Result := Wnd <> 0;
  if not Result then
    Exit;
  PostMessage(Wnd, WM_CLOSE, 0, 0);
  for I := 1 to Tenths do
  begin
    if ByClass then
      Wnd := FindWindowByClassName(Name)
    else
      Wnd := FindWindowByWindowName(Name);
    if Wnd = 0 then
      Break;
    Sleep(100);
  end;
end;

{ Chiude Wheelor in background. Restituisce True se era in esecuzione. }
function CloseWheelor(): Boolean;
begin
  Result := CloseWindow(FindWindowByClassName(MainClass), True, MainClass, 50);
  if Result then
  begin
    Sleep(300); { il processo termina subito dopo la sua finestra }
    Log('Wheelor era in esecuzione: chiuso.');
  end
  else
    Log('Wheelor non era in esecuzione.');
end;

{ Il valore Run punta a questa installazione? (Non tocchiamo quello di altre copie di Wheelor.) }
function IsOwnAutoStart(): Boolean;
var
  Value: String;
begin
  Result := RegQueryStringValue(HKCU, RunKey, '{#AppName}', Value)
    and (Pos(Lowercase(ExpandConstant('{app}\{#AppExe}')), Lowercase(Value)) > 0);
end;

procedure InitializeWizard();
begin
  { Aggiornamento: la casella riflette lo stato attuale, che si può cambiare anche dalle impostazioni. }
  if RegKeyExists(HKCU, UninstallKey) then
  begin
    if RegValueExists(HKCU, RunKey, '{#AppName}') then
      WizardSelectTasks('autostart')
    else
      WizardSelectTasks('!autostart');
  end;
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  Log('PrepareToInstall: chiusura di Wheelor in background.');
  WasRunning := CloseWheelor();
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
begin
  if CurStep <> ssPostInstall then
    Exit;
  { --background: all'accesso nessuna notifica di benvenuto. }
  if WizardIsTaskSelected('autostart') then
    RegWriteStringValue(HKCU, RunKey, '{#AppName}', '"' + ExpandConstant('{app}\{#AppExe}') + '" --background')
  else if IsOwnAutoStart() then
    RegDeleteValue(HKCU, RunKey, '{#AppName}');
  { Aggiornamento silenzioso (es. winget): se Wheelor era attivo, lo riavviamo. }
  if WasRunning and WizardSilent() then
    Exec(ExpandConstant('{app}\{#AppExe}'), '--background', '', SW_SHOWNORMAL, ewNoWait, ResultCode);
end;

function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  CloseWheelor();
  { Finestra delle impostazioni: se ha modifiche non salvate resta aperta a chiedere conferma,
    quindi dopo qualche secondo la chiudiamo d'autorità. }
  if CloseWindow(FindWindowByWindowName('{#SettingsTitle}'), False, '{#SettingsTitle}', 30)
    and (FindWindowByWindowName('{#SettingsTitle}') <> 0) then
    Exec('taskkill.exe', '/F /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep <> usPostUninstall then
    Exit;
  if IsOwnAutoStart() then
    RegDeleteValue(HKCU, RunKey, '{#AppName}');
  { Impostazioni e azioni restano, a meno che l'utente non chieda di eliminarle. }
  if not UninstallSilent() then
    if MsgBox(CustomMessage('DeleteSettings'), mbConfirmation, MB_YESNO or MB_DEFBUTTON2) = IDYES then
      DelTree(ExpandConstant('{userappdata}\{#AppName}'), True, True, True);
end;
