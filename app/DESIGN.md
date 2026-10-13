# Wheelor — Design

App Windows in background (tray) che, premendo una scorciatoia, mostra una ruota di azioni rapide attorno al cursore. Azioni totalmente configurabili.

## Stack

- **Rust** + Win32 API pure tramite il crate ufficiale [`windows`](https://github.com/microsoft/windows-rs).
- Target `x86_64-pc-windows-msvc`, CRT statico → singolo `.exe` (~1 MB), zero runtime.
- Obiettivi: RAM idle ~5–15 MB, CPU idle 0%, apertura ruota < 1 frame.

## Architettura

Un processo:

- **UI thread**: message loop bloccante (`GetMessage`), tray, trigger, overlay, hook temporanei.
- **Esecutore** (`actions.rs`): un thread con una coda, esegue le azioni una alla volta (non deve mai bloccare gli hook; due azioni che premono tasti insieme si mescolerebbero).
- **Watcher thread**: hot reload della config (`ReadDirectoryChangesW` → `PostMessage`).

### Trigger
- Tastiera: `RegisterHotKey` (+ `MOD_NOREPEAT`), nessun hook permanente.
- Tasti mouse (centrale, X1/X2): `WH_MOUSE_LL` permanente con callback minima.
- Modalità: **hold** (rilascio = esegui) e **toggle** (clic = esegui).

### Overlay
- Finestra `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT`: non prende mai il focus.
- HWND in primo piano catturato al momento del trigger (`actions::Context`: finestra a cui torna il passo "Finestra → Torna alla finestra di partenza", base dei profili per app).
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
Un'azione è una sequenza di **passi**, eseguiti uno dopo l'altro (`config::Step`, `actions.rs`):
- `open` (app, file, cartella, indirizzo), `keys`, `text`, `command` (PowerShell / pwsh / cmd, finestra nascosta / visibile / resta aperta, cartella di lavoro, amministratore, con la finestra visibile "aspetta che finisca"; variabili `WHEELOR_APP`, `WHEELOR_TITLE` e `WHEELOR_PROFILE`, non citate nell'interfaccia);
- `wait` (secondi), `wait_window` (finché un'app, o una finestra con un certo titolo, è in primo piano: se è aperta ma dietro la porta davanti; `Apri` torna appena l'app è lanciata, non quando la finestra c'è), `window` (sulla finestra in primo piano: ingrandisci, riduci, ripristina, metà sinistra/destra tenendo conto del bordo invisibile di Windows 10/11, sempre in primo piano, chiudi; oppure torna alla finestra di partenza), `system` (blocca, sospendi, spegni lo schermo, volume, musica), `clipboard` (copia la selezione aspettando che arrivi negli appunti, incolla, metti un testo), `run` (tutti i passi di un'altra azione; i cicli li esclude `Config::check`).

Esecuzione:
- L'esecutore è unico: un'azione avviata mentre un'altra gira aspetta il suo turno. Si ferma al primo passo che non riesce; la notifica dice quale ("Inizio riunione: passo 2 di 3 non riuscito", poi il tipo e il motivo).
- **Esc** ferma l'azione in corso e quelle in coda: mentre l'esecutore lavora (`actions::on_busy` → `WM_ACTIONS_BUSY`) il thread UI installa un `WH_KEYBOARD_LL` che guarda Esc senza mangiarlo (`input::watch_escape`); i tasti inviati dalle azioni stesse (`LLKHF_INJECTED`) non contano. `actions::cancel` fa crescere un contatore: le attese (anche quelle dei comandi e delle finestre) lo controllano spesso.
- Portare una finestra in primo piano (`actions::window::bring_to_front`): Windows lo permette solo a chi ha l'ultimo input, quindi ci si aggancia per un attimo all'input della finestra attuale (`AttachThreadInput`) con un tasto fittizio (VK 0xE8).
- **Segnaposto** nei campi di testo, in inglese come tutto il file (`actions/placeholders.rs`): `{selection}`, `{clipboard}`, `{app}`, `{title}`, `{wheel}`, `{date}` e `{time}` (anche con un formato di Windows: `{date:yyyy-MM-dd}`), `{result}` (l'uscita dell'ultimo comando nascosto) e `{ask:Domanda}` (chiede un testo con una piccola finestra; la stessa domanda una volta sola). Le graffe con un nome sconosciuto restano come sono (`{ $_.Name }` in PowerShell). Il valore entra in modo diverso secondo il campo: in un indirizzo (`https:`, `mailto:`, `ms-teams:`…) codificato; in un comando mai incollato, diventa una variabile d'ambiente (`${env:WHEELOR_SELECTION}` in PowerShell, `!WHEELOR_SELECTION!` in cmd con `/v:on`, solo se servono perché cambia il significato di "!"), così un testo selezionato con dentro `"; del …` resta testo; altrove così com'è. Le `%VARIABILI%` di Windows si espandono prima dei segnaposto. Come amministratore il processo non eredita le variabili: in PowerShell si impostano all'inizio del comando, in cmd i segnaposto non sono permessi.
- `{selection}` si copia all'inizio dell'azione, prima che un passo cambi la finestra in primo piano (la ruota non prende mai il focus): Ctrl+C, attesa del cambio degli appunti (`GetClipboardSequenceNumber`), poi gli appunti di prima vengono rimessi (formato per formato, con "ExcludeClipboardContentFromMonitorProcessing" per non sporcare la cronologia di Win+V). Attenzione: in un terminale Ctrl+C senza selezione interrompe il programma.
- L'uscita dei comandi nascosti si legge su due thread (un comando che scrive molto non si blocca); PowerShell la scrive in UTF-8 (lo chiediamo noi), il resto si decodifica con la codepage della console se non è UTF-8.

Scorciatoie delle azioni: un'azione può avere una sua combinazione (`hotkey`), registrata con `RegisterHotKey` (id da 100 in su, `hotkey::register_actions`): la esegue senza aprire la ruota. Se una non si può registrare (già usata, è quella della ruota…) le altre funzionano e una notifica dice quali no.

### Config
TOML in `%APPDATA%\Wheelor\config.toml`, con chiavi e valori in inglese (anche gli id che crea Wheelor: `action`, `wheel`, `search-selection`…; i nomi mostrati sono nella lingua dell'interfaccia). Ricaricata quando la finestra impostazioni salva (messaggio `WM_RELOAD_CONFIG`) o, se modificata a mano, alla successiva apertura della ruota.

```toml
hotkey = "Ctrl+Alt+W"     # preferenze: prima delle tabelle; scritte solo se diverse dal predefinito
size = "large"            # normal | large | huge
opacity = 85              # 40–100 (%)
accent = "pink"           # aqua | blue | violet | pink | coral | amber | lime | mint
aim_anywhere = true       # si punta anche fuori dalla ruota
check_updates = false     # niente controllo degli aggiornamenti

[[action]]                # libreria: N azioni, id stabile
id = "terminal"
label = "Terminale"
icon = "E756"
type = "open"             # un passo solo: i suoi campi accanto a id e label
target = "wt.exe"

[[action]]
id = "meeting"
label = "Inizio riunione"
hotkey = "Ctrl+Alt+M"     # facoltativa: esegue l'azione senza aprire la ruota

[[action.step]]           # più passi: uno per tabella, nell'ordine
type = "open"
target = "https://teams.microsoft.com/…"

[[action.step]]
type = "wait_window"
app = "ms-teams.exe"
timeout = 15              # secondi; senza, 10

[[action.step]]
type = "window"
do = "left_half"          # focus | minimize | maximize | restore | left_half | right_half | always_on_top | close

[[action.step]]
type = "keys"
keys = "Ctrl+Shift+M"

[[wheel]]                 # "main" esiste sempre ed è la prima
id = "main"
name = "Principale"
slots = ["terminal", "wheel:media"]   # max 8; "wheel:<id>" = sottomenu

[[wheel]]
id = "vscode"
name = "VS Code"
apps = ["code.exe"]       # profilo
slots = ["terminal"]
```

Gli altri passi: `wait` (`seconds = 1.5`), `system` (`do = "lock"`, `sleep`, `screen_off`, `volume_up`, `volume_down`, `mute`, `play_pause`, `next_track`, `previous_track`), `clipboard` (`do = "copy"`, `paste`, `set` con `text = "…"`), `run` (`action = "<id>"`), `command` (con `wait = true` aspetta anche con la finestra visibile).

- Un'azione con un passo solo si scrive "piatta" come nelle prime versioni (file vecchi leggibili senza conversione); `Item` ha `Serialize`/`Deserialize` scritti a mano: legge tutta la tabella e poi decide la forma, così gli errori di un passo arrivano con il loro messaggio ("action `x`: missing field `target`").
- `Config::check` rifiuta id doppi, riferimenti inesistenti (anche nei passi `run`), azioni senza passi, più di 8 elementi e cicli tra sottomenu o tra azioni (`reaches`, `runs_into`): in quel caso resta in uso la configurazione precedente e compare una notifica.
- Il formato della prima versione (`[[slice]]`) viene convertito al caricamento; l'originale resta in `config.v1.toml`.
- Colori di accento (`config::Accent`): otto tinte accese con luminosità e saturazione percepite simili all'acqua del sito (calcolate in OKLCH); ognuna ha le sue varianti (chiara, scura, inchiostro, gradiente della stella), usate dalla ruota e dal `Theme` delle impostazioni.
- Combinazioni di tasti (`keys.rs`): scritte come "Ctrl+Alt+W", con i modificatori sempre nell'ordine Ctrl, Alt, Shift, Win. Oltre ai nomi dei tasti accetta il carattere del tasto nel layout in uso ("ò", "'") e il codice "vkXX" per tutti gli altri.

### Finestra impostazioni
- Stesso exe, processo separato: `wheelor.exe --settings` (il processo in background resta a ~3 MB privati).
- **Slint** (`ui/settings.slint`), backend winit + renderer Skia, che disegna sulla CPU (`skia-software`, scelto in `settings::show`): niente driver della GPU, come la ruota. Il renderer software di Slint costava meno (exe di 7 MB più leggero) ma su schermi al 100% sgranava: testo senza hinting né gamma, bordi da 1 px a scalini sulle curve. Skia rasterizza il testo con DirectWrite. Va compilato da sorgente per il CRT statico (vedi README). Tema scuro con la palette del sito (`Theme`); campi e menu sono i widget Fluent forzati in scuro, pulsanti a pillola, interruttori, scelte a segmenti, righe e finestre di dialogo sono componenti propri, con brevi animazioni (Tab + Invio/Spazio funzionano anche sui pulsanti).
- **Finestra senza cornice e a misura fissa** (`no-frame`, `min-` = `max-width`/`height`): la misura la sceglie `settings::fit_window` all'apertura, la più grande che sta nello schermo con il cursore fino a 1120×780 (non meno di 880×600), e la finestra si apre al centro di quello schermo. La barra del titolo è disegnata (`TitleBar`, alta 44 px sopra le pagine; il logo della colonna di sinistra sta alla stessa altezza): si sposta la finestra trascinandola (`WindowMoveArea` di Slint, che usa `drag_window` di winit), a destra solo riduci a icona (`set_minimized`) e chiudi (rossa sotto il mouse, come in Windows). Chiudere passa da `close_allowed`, come Alt+F4: con modifiche non salvate chiede prima. winit lascia alle finestre senza cornice gli stili di Windows (`WS_CAPTION`, `WS_MINIMIZEBOX`) e toglie solo il bordo in `WM_NCCALCSIZE`: restano l'animazione di riduzione a icona e la chiusura dalla taskbar. Angoli arrotondati e bordo sottile come le altre finestre di Windows 11 si chiedono a DWM (`DWMWA_WINDOW_CORNER_PREFERENCE`, `DWMWA_BORDER_COLOR`; Windows 10 li ignora).
- Griglia: ogni pagina è una colonna (`Page`) con margini di 32 px, larga fino a 1000 px qualunque sia il contenuto, titolo + sezioni (`PageHeader`, `Section`) distanti 32 px; pulsanti e campi alti 32 px; testo a 14 px, spiegazioni a 13, mai sotto i 12.
- Layout adattabile (la misura della finestra dipende dallo schermo): se c'è posto il pannello dell'elemento scelto sta accanto alla ruota, che cresce con lo spazio che avanza fino al 30%, altrimenti sta sotto; nella Libreria l'elenco prende un terzo dello spazio (272–380 px; larghezze preferite fisse 272 e 544, altrimenti il layout partirebbe da quella delle righe e l'elenco resterebbe a 380), e con l'editor sotto i 380 px i suoi pulsanti Duplica ed Elimina restano solo icone. Alla misura minima (880×600) niente esce dai bordi.
- Testi pensati per chiunque, non solo per chi è pratico: una riga di spiegazione solo dove aggiunge qualcosa che titolo e controllo non dicono già, niente gergo ("App collegate" e non "Profilo", "Opzioni di avvio" e non "Argomenti", tipo "Comando" per ultimo).
- Nome e icona di ruote e azioni stanno in cima alla pagina (`NameHeader`): l'icona apre il selettore, il nome si scrive nel titolo stesso (`TitleField`: un `TextInput` che sembra un titolo, largo quanto il testo, con la matita accanto, menu Taglia/Copia/Incolla; Invio o Esc tolgono il focus). Sotto, una riga su quando si apre la ruota (con la scorciatoia scelta) o in quali ruote è l'azione.
- Colonna di sinistra: elenco delle ruote (con il ruolo: principale, profilo, sottomenu, non usata), "Nuova ruota", Libreria, Generale, Informazioni; in fondo, se c'è, il riquadro della nuova versione con "Aggiorna".
- **Ruota**: icona e nome; l'editor (`WheelEditor`), cioè la ruota stessa: niente elenco degli elementi accanto, tutto si fa sull'anteprima circolare, con le stesse misure e lo stesso calcolo della ruota vera (`WheelPreview::hit` rifà `overlay::slice_at` con la zona morta `overlay::DEAD_ZONE`). Accanto (sotto, se lo spazio è poco) il pannello dell'elemento scelto: pallina, "Elemento 2 di 6" (= tasto rapido), nome, cosa fa; Modifica l'azione (o Apri il sottomenu), Sostituisci, Prima/Dopo, Togli. Senza niente di scelto: come si usa, "Aggiungi un'azione" e "Sottomenu". Poi app collegate (il profilo: scritte o scelte tra le finestre aperte); in fondo "Elimina ruota".
  - **Sulla ruota**: finché c'è posto, dopo l'ultima pallina c'è il «+» (che quindi nell'editor occupa un posto in più che sulla ruota vera); apre la finestra «Aggiungi». Clic sceglie, doppio clic modifica, tasto destro apre il menu della pallina (Modifica, Sostituisci…, Sposta prima/dopo, Togli dalla ruota) o, sul «+» e sul resto del disco, quello per aggiungere (un'azione, un sottomenu, Nuova azione, Nuovo sottomenu). I due `ContextMenuArea` sono spenti e li apre l'anteprima con `show()`: sa su quale pallina si è cliccato. Con il focus sulla ruota: frecce per scegliere, Ctrl+frecce per spostare, Canc per togliere, Invio per modificare, Ins o + per aggiungere, il tasto Menu (o Maiusc+F10) per il menu della pallina scelta.
  - **Finestra «Aggiungi»** (`adding`: 1 in fondo, 2 al posto della pallina scelta): schede Azioni e Sottomenu (sostituendo si parte da quella dello stesso tipo), ricerca, "Già nella ruota"; i sottomenu che creerebbero un ciclo non compaiono. Un clic su una voce la mette nella ruota; "Nuova azione" e "Nuovo sottomenu" la creano e la mettono lì (`Place`: in fondo o al posto dell'elemento). Le finestre di dialogo ignorano per 400 ms i clic fuori dal riquadro, che altrimenti le chiuderebbero: era il secondo clic di un doppio clic sul «+».
  - **Trascinamento**: una pallina trascinata si sposta (le altre fanno posto, un anello mostra dove andrà), o fuori dal disco (disco e "fantasma" rossi) si toglie. Fatto con `TouchArea` e non con `DragArea`/`DropArea` di Slint (il loro contenuto è un `data-transfer` opaco da costruire in Rust): chi viene trascinato scrive nel global `Drag` posizione e aspetto, `Ghost` lo disegna sopra a tutto, e l'editor calcola dalla posizione (`absolute-position`) cosa succederebbe rilasciando; l'anteprima mostra lo stesso calcolo (`lift`, `drop-at`, `removing`). Le misure dell'editor sono calcolate a mano, senza layout: così ogni parte sa dov'è l'altra. Rilasciando, i dati cambiano già nel nuovo ordine e le palline devono restare dove sono: per un attimo l'animazione è a 0 ms (`WheelPreview::settle`). Le `ScrollView` Fluent non scorrono trascinando con il mouse, quindi non disturbano. Un `FocusScope` sopra una `TouchArea` le ruba il primo clic per prendersi il focus: lì `focus-on-click: false`, e il focus lo dà la `TouchArea`.
- **Libreria**: tutte le azioni con ricerca (quella selezionata resta in vista: l'elenco ricalcola altezza e scorrimento solo ridisegnando le righe, quindi lo scorrimento si riprova finché la riga nuova non c'è), Nuova (finestra "Nuova azione": un primo passo tra Apri, Tasti, Testo, Comando, Sistema, Finestra, oppure un esempio pronto che mostra passi e segnaposto), editor: icona e nome; **Cosa fa**: i passi uno sotto l'altro, quello scelto aperto con i suoi campi (cambia tipo, sposta, duplica, elimina; con l'editor stretto solo cambia tipo ed elimina), riordinabili trascinandoli (mentre se ne trascina uno sono tutti chiusi e alti uguali, una linea mostra dove andrà); "Aggiungi passo" apre la scelta del tipo (dieci, con una riga di spiegazione); nei campi di testo il menu "{ } Segnaposto" aggiunge un segnaposto in fondo (Slint non dice dov'è il cursore); **Scorciatoia** dell'azione; in fondo "Prova azione" (se l'azione lavora sull'app in primo piano parte dopo 3 secondi, il tempo di passare a quell'app), Duplica ed Elimina (eliminando, l'azione sparisce anche dalle ruote e dai passi che la eseguivano). Arrivando da una ruota ("Modifica", "Nuova azione"), in alto "Torna a «…»".
- **Annulla**: le modifiche alla struttura (elementi delle ruote, passi, azioni e ruote create o eliminate) passano da `App::change`, che tiene una copia della configurazione di prima (fino a 50): nella barra in basso la descrizione e "Annulla", anche con Ctrl+Z (un `FocusScope` attorno a tutto; nei campi di testo vale il loro Ctrl+Z). Scrivere in un campo svuota la cronologia: tornare a una copia precedente cancellerebbe anche quel testo.
- **Generale**: Ruota (scorciatoia, punta da tutto lo schermo), Aspetto (colore, opacità, dimensione: normale, grande +15%, molto grande +30%, applicata moltiplicando i DPI), Sistema (lingua, avvio con Windows, cerca aggiornamenti, file delle impostazioni). Il colore si vede subito in tutta la finestra; la ruota lo usa dopo il salvataggio.
- **Informazioni**: versione con il collegamento a wheelor.app (`/it/` in italiano), stato degli aggiornamenti ("Controlla ora", "Novità", "Aggiorna"; nello Store solo la versione), "Sviluppato da" con il segno di gabrieleiovino.it (`AuthorMark`, gli stessi tracciati del footer del sito) e il collegamento al sito, crediti (icone, il widget AboutSlint).
- Combinazioni di tasti (scorciatoia della ruota, passi `keys`, scorciatoie delle azioni): si clicca il campo e si premono i tasti (`settings/capture.rs`). I tasti premuti insieme formano la combinazione; dopo 2 secondi senza tasti premuti (una barra nel campo li mostra) è quella scelta. Tasti premuti dopo aver rilasciato tutto ne iniziano una nuova: conta l'ultima. Mentre ascolta, un `WH_KEYBOARD_LL` riceve i tasti prima di tutti e li mangia, così si registrano anche Win, Stamp, i tasti multimediali e le combinazioni già usate (compresa quella della ruota) senza attivarle; tasti mangiati anche al rilascio (niente menu Start). Windows non chiama l'hook di un processo registrato al Raw Input della tastiera quando la sua finestra è in primo piano: winit lo registra (per dei DeviceEvent che Slint non usa), quindi prima di ascoltare la registrazione della tastiera si toglie. L'hook ha un thread suo, così i tasti di tutto il sistema non aspettano che Skia finisca di ridisegnare. Esc da solo annulla, un clic altrove anche; con un'altra finestra in primo piano non mangia nulla. Per le scorciatoie (della ruota e delle azioni): rifiutate quelle che toglierebbero un tasto per scrivere (senza Ctrl, Alt o Win, salvo F1–F24, Pausa, tasti multimediali…) quelle già registrate da altri (`hotkey::is_free`, prova `RegisterHotKey`; quelle salvate le ha registrate Wheelor stesso e vanno bene) e quelle già usate dalla ruota o da un'altra azione. Per le azioni, pulsanti per Esc e i tasti multimediali.
- Le modifiche lavorano su una copia della configurazione in memoria (`App::config`); "Salva" la controlla (`Config::check`) e la scrive.
- Eliminare una ruota o un'azione chiede conferma, dicendo da quali ruote sparirà (Esc annulla, Invio conferma). "Togli" un elemento da una ruota non chiede conferma: non elimina nulla.
- Selettore icone: 1533 glifi Segoe Fluent Icons con nome (`src/icons.txt`, dalla documentazione Microsoft) e ricerca anche in italiano.
- I provini (`cargo test settings_snapshots -- --ignored`) si disegnano in memoria con il renderer software di Slint, l'unico usabile fuori da una finestra: sono un po' meno fini della finestra vera, e lì i Path ignorano i gradienti e i gradienti non vengono ritagliati da `border-radius` (per questo i cerchi del logo sono Path). Alcuni simulano il mouse (`click`, anche con il tasto destro, `drag`, `release`) e si disegnano a metà trascinamento o con il menu aperto; l'ultimo è la finestra di `--ask`. La misura fissa della finestra si cambia con `resize`.
- Testi che vanno a capo dentro un layout facoltativo (`if …: VerticalLayout { … Text … }`, anche un componente che eredita da un layout come `Section` o `SettingRow`): Slint li misura su una riga sola e ciò che segue ci finisce sopra (così "Prova azione" copriva la spiegazione sotto il comando). Un testo facoltativo va messo direttamente nel layout (`if …: Text`); dentro un blocco facoltativo, solo testi brevi, su una riga. I provini `libreria-nuova` e `ruote-media-intera` sono alti apposta per vedere tutta la pagina.
- Skia applica `opacity` al gruppo (l'elemento con tutti i figli), il renderer software a ogni elemento: dove conta l'aspetto "sbiadito" di ciascun pezzo (pulsanti disattivati) l'opacità va messa sui singoli elementi.

### Lingue
- Italiano e inglese (`i18n.rs`). Preferenza `language` nel file (`"it"`, `"en"`; assente = come Windows: italiano se l'interfaccia di Windows è in italiano, altrimenti inglese). Al primo avvio la configurazione di esempio nasce nella lingua di Windows.
- Finestra delle impostazioni: i testi di `ui/settings.slint` sono in italiano dentro `@tr(…)`; la traduzione sta in `translations/en/LC_MESSAGES/wheelor.po`, incorporata nell'exe da `build.rs` (`with_bundled_translations`). Cambiando lingua nella pagina Generale la finestra si traduce subito (`slint::select_bundled_translation`), la ruota e la tray dopo il salvataggio. Per un testo nuovo: scriverlo con `@tr("…")` e aggiungere la voce inglese al `.po` (le graffe vanno raddoppiate: `@tr("{{ask:…}}")`; lo stesso testo con significati diversi vuole un contesto: `@tr("Undo" => "Annulla")`) (`slint-tr-extractor ui/settings.slint -o wheelor.pot` elenca tutte le voci, con il contesto = nome del componente).
- Testi preparati in Rust (notifiche, menu, errori, righe degli elenchi, nomi dei tasti): macro `tr!("…", "…")` e `trf!` (come `format!`).
- Il titolo della finestra esiste nelle due lingue: `TITLES` in `settings.rs` e `SettingsTitle`/`SettingsTitleEn` nell'installer lo cercano entrambi.

### Logo
`assets/logo.svg` → `tools/make-icon` genera `assets/wheelor.ico` (10 dimensioni), incorporata come risorsa 1 da `build.rs` (winresource). Usata da exe, tray e finestra impostazioni; la stella del logo è disegnata al centro della ruota.

### Avvio e distribuzione
- Riga di comando: `wheelor.exe` (avvia; se è già attivo apre le impostazioni), `--background` (avvio automatico, senza notifica di benvenuto; con `--updated` annuncia l'aggiornamento riuscito), `--settings` (con `--update` propone subito di aggiornare), `--quit` (chiude l'istanza attiva e aspetta), `--run <id>` (esegue un'azione della libreria, per Stream Deck, collegamenti e script: se Wheelor è attivo gliela passa con `WM_COPYDATA`, così Esc la ferma e gli errori arrivano come notifica; altrimenti la esegue e aspetta, con gli errori in un messaggio), `--ask <domanda>` (la finestra dei segnaposto `{ask:…}`: compare vicino al cursore, sopra a tutto, e scrive la risposta su stdout; esce con 1 se annullata).
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
10. ✅ Azioni a passi (attese, finestre, sistema, appunti, altre azioni), segnaposto, scorciatoie delle azioni, `--run`; impostazioni con la ruota come editor (il «+», menu del tasto destro, finestra «Aggiungi», trascinamento), passi riordinabili, esempi pronti, Annulla; finestra a misura fissa con la barra del titolo disegnata.
11. Più avanti: azioni a interruttore (due sequenze che si alternano, con lo stato sullo spicchio), ruote dinamiche (finestre aperte, file di una cartella), registratore di tasti, tema chiaro.

## Decisioni
- Hotkey di default: **Ctrl+Alt+W** (Ctrl+Alt+Space è già usato da Claude).
- Modalità **ibrida**, senza impostazioni:
  - pressione rapida → la ruota resta aperta, si sceglie con un clic (clic sul mozzo, tasto destro, Esc, clic fuori dalla ruota o nuova pressione della hotkey = annulla);
  - hotkey tenuta → si punta uno spicchio, al rilascio parte l'azione (rilascio nel mozzo = la ruota resta aperta).
  - pressione rapida, poi Spazio → esegue lo spicchio puntato (come il clic).
  - Tasti 1–9 selezionano direttamente lo spicchio.

## Decisioni aperte
- Supporto ai tasti mouse come trigger?
