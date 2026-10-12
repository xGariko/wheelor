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
- Aspetto (palette del sito): disco scuro, azioni come palline chiare su un'orbita (come nel logo), stella al centro. Lo spicchio puntato si accende d'accento con un alone; un arco sull'anello del mozzo indica la direzione; il mozzo mostra il nome dello spicchio puntato. Etichette sotto le palline (con "…" se lunghe); senza icona, l'iniziale.
- Animazioni: apertura (dissolvenza + leggero ingrandimento, palline che escono dal centro una dopo l'altra, stella che "scatta"), cambio di spicchio (accensione, arco e nome che scorrono), chiusura annullata (dissolvenza di 120 ms). Tornando indietro le palline rientrano verso l'orbita. Quando parte un'azione la ruota sparisce subito (niente ruota negli screenshot).
- Timer dei fotogrammi (~60 al secondo) attivo solo mentre qualcosa si muove; ~2–3 ms per fotogramma a 150%. La selezione non dipende mai dall'animazione. Con gli effetti di animazione di Windows disattivati, tutto è immediato.
- Provini dell'aspetto: `cargo test snapshots -- --ignored` scrive `target/wheel.bmp` (vari momenti delle animazioni).
- Pre-creata nascosta all'avvio. Per-Monitor DPI v2. Clamp dentro il monitor del cursore.
- Icone: glifi Segoe Fluent Icons, icone exe (`SHGetFileInfo`), PNG/ICO via WIC.

### Input a ruota aperta
- `WH_MOUSE_LL` + `WH_KEYBOARD_LL` installati solo mentre la ruota è visibile: clic catturati e consumati ovunque, 1–8 selezionano, rilascio del trigger in modalità hold.
- Esc chiude tutto; clic sul mozzo, tasto destro e Backspace tornano alla ruota precedente (dalla prima chiudono).
- Spazio sceglie lo spicchio puntato, come il clic sinistro (nel mozzo non fa nulla; se la hotkey contiene lo Spazio, resta la hotkey). L'autorepeat è ignorato: tenendolo premuto non sceglie anche nel sottomenu appena aperto.
- Selezione per direzione: `atan2` dal centro + zona morta.

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

### Finestra impostazioni
- Stesso exe, processo separato: `wheelor.exe --settings` (il processo in background resta a ~3 MB privati).
- **Slint** (`ui/settings.slint`), backend winit + renderer software. Tema scuro con la palette del sito (`Theme`); campi e menu sono i widget Fluent forzati in scuro, pulsanti a pillola, interruttori, scelte a segmenti, righe e finestre di dialogo sono componenti propri, con brevi animazioni (Tab + Invio/Spazio funzionano anche sui pulsanti).
- Griglia: ogni pagina è una colonna (`Page`) con margini di 32 px e larghezza massima di 1000 px, titolo + sezioni (`PageHeader`, `Section`) distanti 32 px; pulsanti e campi alti 32 px; testo a 14 px, spiegazioni a 13, mai sotto i 12.
- Layout adattabile: con almeno 840 px di contenuto l'anteprima della ruota sta accanto all'elenco degli elementi e cresce fino al 20%, altrimenti sta sopra; nella Libreria l'elenco prende un terzo dello spazio (272–380 px).
- Colonna di sinistra: elenco delle ruote (con il ruolo: principale, profilo, sottomenu, non usata), "Nuova ruota", Libreria, Generale.
- **Ruota**: nome, icona, app del profilo (scritte o scelte tra le finestre aperte), anteprima circolare cliccabile con le stesse misure e lo stesso calcolo della ruota vera (`overlay::slice_at`, `overlay::DEAD_ZONE`), elenco degli elementi (l'elemento selezionato mostra le sue azioni: sposta, modifica, togli); aggiungi azione/sottomenu da una finestra di scelta con ricerca (i sottomenu che creerebbero un ciclo non compaiono), riordina, togli, "Modifica" salta all'azione o alla ruota.
- **Libreria**: tutte le azioni con ricerca, Nuova / Duplica / Elimina (eliminando, l'azione sparisce anche dalle ruote), editor (etichetta, icona, tipo e parametri, "Prova azione", ruote in cui è usata).
- **Generale**: scorciatoia, dimensione della ruota (normale, grande +15%, molto grande +30%: `size` nel file, applicata moltiplicando i DPI), avvio con Windows, percorso della configurazione.
- Le modifiche lavorano su una copia della configurazione in memoria (`App::config`); "Salva" la controlla (`Config::check`) e la scrive.
- Eliminare una ruota o un'azione chiede conferma, dicendo da quali ruote sparirà (Esc annulla, Invio conferma). "Togli" un elemento da una ruota non chiede conferma: non elimina nulla.
- Selettore icone: 1533 glifi Segoe Fluent Icons con nome (`src/icons.txt`, dalla documentazione Microsoft) e ricerca anche in italiano.
- Limite noto del renderer software: i Path ignorano i gradienti e i gradienti non vengono ritagliati da `border-radius`.

### Logo
`assets/logo.svg` → `tools/make-icon` genera `assets/wheelor.ico` (10 dimensioni), incorporata come risorsa 1 da `build.rs` (winresource). Usata da exe, tray e finestra impostazioni; la stella del logo è disegnata al centro della ruota.

### Avvio e distribuzione
- Riga di comando: `wheelor.exe` (avvia; se è già attivo apre le impostazioni), `--background` (avvio automatico, senza notifica di benvenuto), `--settings`, `--quit` (chiude l'istanza attiva e aspetta).
- Autostart: `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` = `"…\wheelor.exe" --background`.
- Single instance: mutex `Local\Wheelor.SingleInstance` (condiviso da build di sviluppo e versione installata: per provare una build di sviluppo chiudere prima quella installata con `--quit`).
- Exe: manifest (PerMonitorV2, asInvoker, Common Controls 6, Windows 10/11) e informazioni di versione da `build.rs`. Su Windows 10 le icone usano Segoe MDL2 Assets e il testo Segoe UI.
- Robustezza: se la tray non è pronta all'accesso, nuovo tentativo ogni 2 s; `WM_ENDSESSION` chiude in modo pulito; le risorse Direct2D si liberano prima di uscire (`overlay::shutdown`, altrimenti crash alla chiusura); i panic finiscono in `%APPDATA%\Wheelor\crash.log` con un messaggio all'utente.
- **Installer** (`installer/wheelor.iss`, Inno Setup 6.6+), creato da `installer/build.ps1` → `dist/Wheelor-Setup-<versione>.exe` + `.sha256` + `latest.json` (versione, data, indirizzo, peso e hash: li usa il sito):
  - per l'utente corrente, senza amministratore, in `%LOCALAPPDATA%\Programs\Wheelor`; italiano e inglese; tema chiaro/scuro come Windows; immagini generate da `tools/make-icon`;
  - attività "Avvia all'accesso" (in aggiornamento riflette lo stato attuale); collegamento nel menu Start; voce in Impostazioni > App;
  - aggiornamento: chiude Wheelor in background (`WM_CLOSE` alla finestra `WheelorMain`), la finestra delle impostazioni la chiude la Gestione riavvio; in modalità silenziosa (winget) riavvia Wheelor se era attivo;
  - disinstallazione: chiude Wheelor, toglie l'avvio automatico se punta a questa installazione, chiede se eliminare le impostazioni (predefinito: no; in silenzioso le conserva).
- **Release e sito** (vedi il `README.md` nella radice del repository): un tag `vX.Y.Z` fa partire `.github/workflows/release.yml`, che compila l'installer su Windows e lo pubblica come GitHub Release; poi `site.yml` ricrea la landing page (`../web`) con i dati di `latest.json` e la pubblica su GitHub Pages, all'indirizzo https://wheelor.app.
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
6. Pubblicazione: ✅ landing page e release automatiche; da fare: licenza, avvisi di terze parti, firma, winget.
7. ✅ Nuovo aspetto di ruota e impostazioni (palette del sito), animazioni, conferma delle eliminazioni.
8. Più avanti: interfaccia in inglese, azioni `window`/`system`, tema chiaro.

## Decisioni
- Hotkey di default: **Ctrl+Alt+W** (Ctrl+Alt+Space è già usato da Claude).
- Modalità **ibrida**, senza impostazioni:
  - pressione rapida → la ruota resta aperta, si sceglie con un clic (clic sul mozzo, tasto destro, Esc o nuova pressione della hotkey = annulla);
  - hotkey tenuta → si punta uno spicchio, al rilascio parte l'azione (rilascio nel mozzo = la ruota resta aperta).
  - pressione rapida, poi Spazio → esegue lo spicchio puntato (come il clic).
  - Tasti 1–9 selezionano direttamente lo spicchio.

## Decisioni aperte
- Supporto ai tasti mouse come trigger?
