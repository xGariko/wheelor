# Wheelor — Design

App Windows in background (tray) che, premendo una scorciatoia, mostra una ruota di azioni rapide attorno al cursore. Azioni totalmente configurabili.

## Stack

- **Rust** + Win32 API pure tramite il crate ufficiale [`windows`](https://github.com/microsoft/windows-rs).
- Target `x86_64-pc-windows-msvc`, CRT statico → singolo `.exe` (~1 MB), zero runtime.
- Obiettivi: RAM idle ~5–15 MB, CPU idle 0%, apertura ruota < 1 frame.

## Architettura

Un processo:

- **UI thread**: message loop bloccante (`GetMessage`), tray, trigger, overlay, hook temporanei.
- **Worker thread**: esecuzione azioni (non deve mai bloccare gli hook).
- **Watcher thread**: hot reload della config (`ReadDirectoryChangesW` → `PostMessage`).

### Trigger
- Tastiera: `RegisterHotKey` (+ `MOD_NOREPEAT`), nessun hook permanente.
- Tasti mouse (centrale, X1/X2): `WH_MOUSE_LL` permanente con callback minima.
- Modalità: **hold** (rilascio = esegui) e **toggle** (clic = esegui).

### Overlay
- Finestra `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`: non prende mai il focus.
- HWND in primo piano catturato al momento del trigger (target delle azioni `window`, base dei profili per app).
- Rendering: Direct2D + DirectWrite su bitmap software → `UpdateLayeredWindow` (alpha per pixel, nessun driver GPU). Upgrade possibile a DirectComposition.
- Aspetto (palette del sito): disco scuro, azioni come palline chiare su un'orbita (come nel logo), stella al centro. Lo spicchio puntato si accende del colore di accento (preferenza, vedi Config) con un alone; un arco sull'anello del mozzo indica la direzione; il mozzo mostra il nome dello spicchio puntato. Etichette sotto le palline (con "…" se lunghe); senza icona, l'iniziale.
- Animazioni: apertura (dissolvenza + leggero ingrandimento, palline che escono dal centro una dopo l'altra, stella che "scatta"), cambio di spicchio (accensione, arco e nome che scorrono), chiusura annullata (dissolvenza di 120 ms). Tornando indietro le palline rientrano verso l'orbita. Quando parte un'azione la ruota sparisce subito (niente ruota negli screenshot).
- Opacità (preferenza, 40–100%): moltiplica l'alpha dell'intera finestra in `UpdateLayeredWindow`, senza ridisegnare.
- Timer dei fotogrammi (~60 al secondo) attivo solo mentre qualcosa si muove; ~2–3 ms per fotogramma a 150%. La selezione non dipende mai dall'animazione. Con gli effetti di animazione di Windows disattivati, tutto è immediato.
- Provini dell'aspetto: `cargo test snapshots -- --ignored` scrive `target/wheel.bmp` (vari momenti delle animazioni).
- Pre-creata nascosta all'avvio. Per-Monitor DPI v2. Clamp dentro il monitor del cursore.
- Icone: glifi Segoe Fluent Icons, icone exe (`SHGetFileInfo`), PNG/ICO via WIC.

### Input a ruota aperta
- `WH_MOUSE_LL` + `WH_KEYBOARD_LL` installati solo mentre la ruota è visibile: clic catturati e consumati ovunque, 1–8 selezionano, rilascio del trigger in modalità hold.
- Esc chiude tutto; clic sul mozzo, tasto destro e Backspace tornano alla ruota precedente (dalla prima chiudono). Clic fuori dalla ruota (con qualunque tasto): chiude, salvo con "Punta da tutto lo schermo".
- Spazio sceglie lo spicchio puntato, come il clic sinistro (nel mozzo non fa nulla; se la hotkey contiene lo Spazio, resta la hotkey). L'autorepeat è ignorato: tenendolo premuto non sceglie anche nel sottomenu appena aperto.
- Selezione per direzione: `atan2` dal centro + zona morta. Oltre il bordo del disco si punta solo con `aim_anywhere` (predefinito: no, lì non si accende nulla e un clic chiude la ruota).

### Sottomenu e profili
- Le ruote aperte sono una pila (`wheel.rs`); ogni ruota si apre centrata sul cursore.
- Un sottomenu si apre con clic, tasto numerico, rilascio della hotkey sullo spicchio o, tenendo premuta la hotkey, con una pausa di 350 ms (il timer riparte a ogni movimento).
- Spicchio-sottomenu: freccetta sul bordo. Mozzo: logo (principale), nome (profilo), freccia indietro + nome (sottomenu).
- Profilo: all'apertura si sceglie la prima ruota con `apps` che contiene l'eseguibile in primo piano (nome del file senza maiuscole, `.exe` facoltativo, oppure percorso completo); altrimenti la principale.

### Azioni
Implementate: `command` (PowerShell / pwsh / cmd, finestra nascosta / visibile / resta aperta, cartella di lavoro, amministratore, variabili `WHEELOR_APP`, `WHEELOR_TITLE` e `WHEELOR_PROFILE`), `open`, `keys`, `text`.
Da fare: `window`, `system`.

### Config
TOML in `%APPDATA%\Wheelor\config.toml`. Ricaricata quando la finestra impostazioni salva (messaggio `WM_RELOAD_CONFIG`) o, se modificata a mano, alla successiva apertura della ruota.

```toml
hotkey = "Ctrl+Alt+W"     # preferenze: prima delle tabelle; scritte solo se diverse dal predefinito
size = "large"            # normal | large | huge
opacity = 85              # 40–100 (%)
accent = "pink"           # aqua | blue | violet | pink | coral | amber | lime | mint
aim_anywhere = true       # si punta anche fuori dalla ruota
check_updates = false     # niente controllo degli aggiornamenti

[[action]]                # libreria: N azioni, id stabile
id = "terminale"
label = "Terminale"
icon = "E756"
type = "open"
target = "wt.exe"

[[wheel]]                 # "main" esiste sempre ed è la prima
id = "main"
name = "Principale"
slots = ["terminale", "wheel:media"]   # max 8; "wheel:<id>" = sottomenu

[[wheel]]
id = "vscode"
name = "VS Code"
apps = ["code.exe"]       # profilo
slots = ["terminale"]
```

- `Config::check` rifiuta id doppi, riferimenti inesistenti, più di 8 elementi e cicli tra sottomenu: in quel caso resta in uso la configurazione precedente e compare una notifica.
- Il formato della prima versione (`[[slice]]`) viene convertito al caricamento; l'originale resta in `config.v1.toml`.
- Colori di accento (`config::Accent`): otto tinte accese con luminosità e saturazione percepite simili all'acqua del sito (calcolate in OKLCH); ognuna ha le sue varianti (chiara, scura, inchiostro, gradiente della stella), usate dalla ruota e dal `Theme` delle impostazioni.
- Combinazioni di tasti (`keys.rs`): scritte come "Ctrl+Alt+W", con i modificatori sempre nell'ordine Ctrl, Alt, Shift, Win. Oltre ai nomi dei tasti accetta il carattere del tasto nel layout in uso ("ò", "'") e il codice "vkXX" per tutti gli altri.

### Finestra impostazioni
- Stesso exe, processo separato: `wheelor.exe --settings` (il processo in background resta a ~3 MB privati).
- **Slint** (`ui/settings.slint`), backend winit + renderer software. Tema scuro con la palette del sito (`Theme`); campi e menu sono i widget Fluent forzati in scuro, pulsanti a pillola, interruttori, scelte a segmenti, righe e finestre di dialogo sono componenti propri, con brevi animazioni (Tab + Invio/Spazio funzionano anche sui pulsanti).
- Griglia: ogni pagina è una colonna (`Page`) con margini di 32 px e larghezza massima di 1000 px, titolo + sezioni (`PageHeader`, `Section`) distanti 32 px; pulsanti e campi alti 32 px; testo a 14 px, spiegazioni a 13, mai sotto i 12.
- Layout adattabile: con almeno 840 px di contenuto l'anteprima della ruota sta accanto all'elenco degli elementi e cresce fino al 20%, altrimenti sta sopra; nella Libreria l'elenco prende un terzo dello spazio (272–380 px).
- Colonna di sinistra: elenco delle ruote (con il ruolo: principale, profilo, sottomenu, non usata), "Nuova ruota", Libreria, Generale, Informazioni; in fondo, se c'è, il riquadro della nuova versione con "Aggiorna".
- **Ruota**: nome, icona, app del profilo (scritte o scelte tra le finestre aperte), anteprima circolare cliccabile con le stesse misure e lo stesso calcolo della ruota vera (`overlay::slice_at`, `overlay::DEAD_ZONE`), elenco degli elementi (l'elemento selezionato mostra le sue azioni: sposta, modifica, togli); aggiungi azione/sottomenu da una finestra di scelta con ricerca (i sottomenu che creerebbero un ciclo non compaiono), riordina, togli, "Modifica" salta all'azione o alla ruota.
- **Libreria**: tutte le azioni con ricerca, Nuova / Duplica / Elimina (eliminando, l'azione sparisce anche dalle ruote), editor (etichetta, icona, tipo e parametri, "Prova azione", ruote in cui è usata).
- **Generale**: Ruota (scorciatoia, punta da tutto lo schermo), Aspetto (colore, opacità, dimensione: normale, grande +15%, molto grande +30%, applicata moltiplicando i DPI), Sistema (avvio con Windows, cerca aggiornamenti, percorso della configurazione). Il colore si vede subito in tutta la finestra; la ruota lo usa dopo il salvataggio.
- **Informazioni**: versione, stato degli aggiornamenti ("Controlla ora", "Novità", "Aggiorna"), il widget AboutSlint.
- Combinazioni di tasti (scorciatoia e azioni `keys`): si clicca il campo e si premono i tasti (`settings/capture.rs`). Mentre ascolta, un `WH_KEYBOARD_LL` riceve i tasti prima di tutti e li mangia, così si registrano anche Win, Stamp, i tasti multimediali e le combinazioni già usate (compresa quella della ruota) senza attivarle; tasti mangiati anche al rilascio (niente menu Start). Esc da solo annulla, un clic altrove anche; con un'altra finestra in primo piano non mangia nulla. Per la scorciatoia: rifiutate quelle che toglierebbero un tasto per scrivere (senza Ctrl, Alt o Win, salvo F1–F24, Pausa, tasti multimediali…) e quelle già registrate da altri (`hotkey::is_free`, prova `RegisterHotKey`). Per le azioni, pulsanti per Esc e i tasti multimediali.
- Le modifiche lavorano su una copia della configurazione in memoria (`App::config`); "Salva" la controlla (`Config::check`) e la scrive.
- Eliminare una ruota o un'azione chiede conferma, dicendo da quali ruote sparirà (Esc annulla, Invio conferma). "Togli" un elemento da una ruota non chiede conferma: non elimina nulla.
- Selettore icone: 1533 glifi Segoe Fluent Icons con nome (`src/icons.txt`, dalla documentazione Microsoft) e ricerca anche in italiano.
- Limite noto del renderer software: i Path ignorano i gradienti e i gradienti non vengono ritagliati da `border-radius`.

### Lingue
- Italiano e inglese (`i18n.rs`). Preferenza `language` nel file (`"it"`, `"en"`; assente = come Windows: italiano se l'interfaccia di Windows è in italiano, altrimenti inglese). Al primo avvio la configurazione di esempio nasce nella lingua di Windows.
- Finestra delle impostazioni: i testi di `ui/settings.slint` sono in italiano dentro `@tr(…)`; la traduzione sta in `translations/en/LC_MESSAGES/wheelor.po`, incorporata nell'exe da `build.rs` (`with_bundled_translations`). Cambiando lingua nella pagina Generale la finestra si traduce subito (`slint::select_bundled_translation`), la ruota e la tray dopo il salvataggio. Per un testo nuovo: scriverlo con `@tr("…")` e aggiungere la voce inglese al `.po` (`slint-tr-extractor ui/settings.slint -o wheelor.pot` elenca tutte le voci, con il contesto = nome del componente).
- Testi preparati in Rust (notifiche, menu, errori, righe degli elenchi, nomi dei tasti): macro `tr!("…", "…")` e `trf!` (come `format!`).
- Il titolo della finestra esiste nelle due lingue: `TITLES` in `settings.rs` e `SettingsTitle`/`SettingsTitleEn` nell'installer lo cercano entrambi.

### Logo
`assets/logo.svg` → `tools/make-icon` genera `assets/wheelor.ico` (10 dimensioni), incorporata come risorsa 1 da `build.rs` (winresource). Usata da exe, tray e finestra impostazioni; la stella del logo è disegnata al centro della ruota.

### Avvio e distribuzione
- Riga di comando: `wheelor.exe` (avvia; se è già attivo apre le impostazioni), `--background` (avvio automatico, senza notifica di benvenuto; con `--updated` annuncia l'aggiornamento riuscito), `--settings` (con `--update` propone subito di aggiornare), `--quit` (chiude l'istanza attiva e aspetta).
- Autostart: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` = `"…\wheelor.exe" --background`.
- Single instance: mutex `Local\Wheelor.SingleInstance` (condiviso da build di sviluppo e versione installata: per provare una build di sviluppo chiudere prima quella installata con `--quit`).
- Exe: manifest (PerMonitorV2, asInvoker, Common Controls 6, Windows 10/11) e informazioni di versione da `build.rs`. Su Windows 10 le icone usano Segoe MDL2 Assets e il testo Segoe UI.
- Robustezza: se la tray non è pronta all'accesso, nuovo tentativo ogni 2 s; `WM_ENDSESSION` chiude in modo pulito; le risorse Direct2D si liberano prima di uscire (`overlay::shutdown`, altrimenti crash alla chiusura); i panic finiscono in `%APPDATA%\Wheelor\crash.log` con un messaggio all'utente.
- **Installer** (`installer/wheelor.iss`, Inno Setup 6.6+), creato da `installer/build.ps1` → `dist/Wheelor-Setup-<versione>.exe` + `.sha256` + `latest.json` (versione, data, indirizzo, peso e hash: li usa il sito):
  - per l'utente corrente, senza amministratore, in `%LOCALAPPDATA%\Programs\Wheelor`; italiano e inglese; tema chiaro/scuro come Windows; immagini generate da `tools/make-icon`;
  - attività "Avvia all'accesso" (in aggiornamento riflette lo stato attuale); collegamento nel menu Start; voce in Impostazioni > App;
  - aggiornamento: chiude Wheelor in background (`WM_CLOSE` alla finestra `WheelorMain`), la finestra delle impostazioni la chiude la Gestione riavvio; in modalità silenziosa (aggiornamento dall'app, winget) riavvia Wheelor se era attivo, con `--background --updated`;
  - disinstallazione: chiude Wheelor, toglie l'avvio automatico se punta a questa installazione, chiede se eliminare le impostazioni (predefinito: no; in silenzioso le conserva).
- **Release e sito** (vedi il `README.md` nella radice del repository): un tag `vX.Y.Z` fa partire `.github/workflows/release.yml`, che compila l'installer su Windows e lo pubblica come GitHub Release; poi `site.yml` ricrea la landing page (`../web`) con i dati di `latest.json` e la pubblica su GitHub Pages, all'indirizzo https://wheelor.app.
- **Aggiornamenti** (`update.rs`): Wheelor in background legge `https://wheelor.app/latest.json` 20 s dopo l'avvio e poi ogni 6 ore (se `check_updates`, predefinito sì), con WinHTTP (proxy e certificati di sistema, nessuna libreria in più). Se la versione è più nuova (confronto semver, le pre-release vengono prima della versione finale): notifica "Wheelor X è disponibile" (una volta per versione) e voce "Aggiorna a Wheelor X…" nel menu della tray; entrambe aprono le impostazioni con `--update`. Lì "Aggiorna" salva le modifiche in corso, scarica l'installer (solo da `github.com/xGariko/wheelor/releases/download/`), controlla lo SHA-256 di `latest.json` (BCrypt di Windows) e lo avvia con `/SILENT /SUPPRESSMSGBOXES /NORESTART`; la finestra si chiude, l'installer chiude Wheelor, lo aggiorna e lo riavvia. Gli installer scaricati (`%TEMP%\Wheelor-Setup-*.exe`) si eliminano al controllo successivo.
- **Microsoft Store** (`installer/msix`, `src/package.rs`): stesso exe impacchettato in MSIX con `installer/msix/build.ps1` (icone da `tools/make-icon` in `assets/msix`, identità in `identity.json` copiata da Partner Center, versione X.Y.Z-pre.N → X.Y.(Z·100+N).0 e X.Y.Z → X.Y.(Z·100+99).0 perché lo Store vuole l'ultimo numero a 0). Non firmato: lo firma lo Store. `release.yml` lo crea come artefatto dell'esecuzione quando `identity.json` è compilato. Differenze a runtime (`package::is_store()`: identità del pacchetto o exe sotto `WindowsApps`):
  - niente controllo degli aggiornamenti né pulsante "Aggiorna" (li fa lo Store, le sue regole vietano l'auto-aggiornamento);
  - avvio con Windows: startup task `WheelorStartup` del manifest (attivo da subito, con `--background`), letto e cambiato con l'API `StartupTask` invece della chiave Run;
  - le impostazioni si aprono con `IApplicationActivationManager` (identità del pacchetto garantita);
  - i file nuovi in `%APPDATA%` finiscono in `%LOCALAPPDATA%\Packages\<famiglia>\LocalCache\Roaming`: Wheelor li vede al solito posto, Esplora file no (`package::visible_path` per "Apri cartella" e crash.log). Una configurazione già esistente dell'installer viene letta;
  - richiede Windows 10 2004 o successivo; capability `runFullTrust` (da giustificare all'invio);
  - lingue del pacchetto: inglese (predefinita) e italiano; la descrizione sta in `installer/msix/Strings/<lingua>/Resources.resw`.
- Da fare prima di pubblicare: licenza, nome dell'editore e copyright, avvisi di terze parti (crate Rust, Slint), firma del codice, repository pubblico, manifest winget.

## Insidie note
- **Modificatori ancora premuti** quando si eseguono azioni `keys` → attendere il rilascio o inviare key-up sintetici.
- **Alt/Win orfani** (aprono menu/Start) → iniettare un tasto fittizio (VK 0xE8).
- **UIPI**: hook e `SendInput` non raggiungono finestre elevate → opzione avvio elevato via Task Scheduler.
- **Hook LL**: callback brevissime, altrimenti Windows le rimuove silenziosamente.

## Roadmap
1. ✅ **MVP**: tray + hotkey + ruota + config TOML + azioni.
2. ✅ GUI impostazioni (Slint), logo.
3. ✅ Libreria di azioni + ruote da 8, sottomenu, profili per app (lato ruota).
4. ✅ Impostazioni rifatte: Ruote (anteprima circolare, app associate) / Libreria / Generale.
5. ✅ Installer per-utente (Inno Setup), `--quit`/`--background`, compatibilità Windows 10, crash log.
6. Pubblicazione: ✅ landing page (inglese e italiano, con informativa sulla privacy) e release automatiche; da fare: licenza, avvisi di terze parti, firma, winget.
7. ✅ Nuovo aspetto di ruota e impostazioni (palette del sito), animazioni, conferma delle eliminazioni.
8. ✅ Scorciatoie registrate premendo i tasti, colore di accento, opacità, clic fuori dalla ruota, aggiornamenti dall'app.
9. ✅ Interfaccia in inglese (lingua selezionabile), versione per il Microsoft Store.
10. Più avanti: azioni `window`/`system`, tema chiaro.

## Decisioni
- Hotkey di default: **Ctrl+Alt+W** (Ctrl+Alt+Space è già usato da Claude).
- Modalità **ibrida**, senza impostazioni:
  - pressione rapida → la ruota resta aperta, si sceglie con un clic (clic sul mozzo, tasto destro, Esc, clic fuori dalla ruota o nuova pressione della hotkey = annulla);
  - hotkey tenuta → si punta uno spicchio, al rilascio parte l'azione (rilascio nel mozzo = la ruota resta aperta).
  - pressione rapida, poi Spazio → esegue lo spicchio puntato (come il clic).
  - Tasti 1–9 selezionano direttamente lo spicchio.

## Decisioni aperte
- Supporto ai tasti mouse come trigger?
