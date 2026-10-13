# Changelog

Le novità di ogni versione di Wheelor, in inglese e in italiano. La sezione di una versione diventa
la descrizione della sua release su GitHub (vedi `.github/workflows/release.yml`), che è anche la
pagina aperta da «Novità» nell'app: va scritta prima di creare il tag.

## 0.3.1-beta.1 — 2026-10-13

**English**
- Fixed: with both the Microsoft Store version and the installer version installed, opening Wheelor could show "Update" and run the installer. Now the Store version is in charge: the installer copy hands over to it as soon as it starts and no longer updates itself, and the Store version closes an older installer copy that's still running.

**Italiano**
- Corretto: con installate sia la versione del Microsoft Store sia quella dell'installer, aprendo Wheelor poteva comparire «Aggiorna», che avviava l'installer. Ora comanda la versione dello Store: la copia dell'installer le passa la mano appena parte e non si aggiorna più da sola, e la versione dello Store chiude una copia dell'installer più vecchia ancora attiva.

## 0.3.0-beta.1 — 2026-10-13

**English**

Actions grow into small automations, and the wheel itself becomes the place where you build them.

**Actions**
- An action can now be a sequence of steps, run one after another.
- New steps: **Wait** a few seconds, **Wait for an app** (or a window with a given title) to come to the front, **Window** (maximize, minimize, restore, left or right half, always on top, close, back to the window you started from), **System** (lock, sleep, screen off, volume, play/pause, next and previous track), **Clipboard** (copy the selection, paste, put in a text) and **Run another action**.
- Placeholders in text fields: `{selection}`, `{clipboard}`, `{app}`, `{title}`, `{wheel}`, `{date}`, `{time}`, `{result}` (what the previous command printed) and `{ask:…}`, which asks you for a text when the action runs. They go in safely: encoded inside web addresses, never turned into code inside commands.
- Every action can have its own keyboard shortcut and run without opening the wheel.
- `wheelor.exe --run <id>` runs an action from a Stream Deck, a shortcut or a script.
- Actions run one at a time, and **Esc** stops the one running (and any waiting). If a step fails, the notification says which one and why.
- **Try action** waits 3 seconds when the action works on the app in front, so you have time to switch to it.

**Settings**
- The wheel is the editor: click a ball to see what it does, double-click to edit it, right-click for every command, drag it to move it, or drag it out of the disc to remove it. The **+** after the last ball adds an action or a submenu. The keyboard works too: arrows, Ctrl+arrows, Delete, Enter, Insert.
- A new **Add** window, with Actions and Submenus, search, and **New action** / **New submenu** right there.
- **New action** starts from a step or from one of eight ready-made examples: search or translate the selection, search on YouTube, paste as plain text, email the selection, type today's date, two apps side by side, take a break.
- Steps can be reordered by dragging them; text fields have a **{ } Placeholder** menu.
- **Undo**, also with Ctrl+Z, for changes to wheels, steps and actions.
- The window opens at a size that fits your screen, centred on the screen with the cursor, with its own title bar and the rounded corners of Windows 11.
- Recording a shortcut: press the keys together and let go; the combination is picked after 2 seconds.
- Fixed: recording a shortcut could pick up no keys at all, or stop listening halfway.
- The About page links to wheelor.app and lists the credits.
- Clearer wording throughout, written for everyone.

**Under the hood**
- New settings files use English ids (`terminal`, `search-selection`…). Files from earlier versions keep working as they are.

**Italiano**

Le azioni diventano piccole automazioni, e la ruota stessa diventa il posto dove costruirle.

**Azioni**
- Un'azione ora può essere una sequenza di passi, eseguiti uno dopo l'altro.
- Nuovi passi: **Aspetta** qualche secondo, **Aspetta un'app** (o una finestra con un certo titolo) in primo piano, **Finestra** (ingrandisci, riduci, ripristina, metà sinistra o destra, sempre in primo piano, chiudi, torna alla finestra di partenza), **Sistema** (blocca, sospendi, spegni lo schermo, volume, play/pausa, brano successivo e precedente), **Appunti** (copia la selezione, incolla, metti un testo) ed **Esegui un'altra azione**.
- Segnaposto nei campi di testo: `{selection}`, `{clipboard}`, `{app}`, `{title}`, `{wheel}`, `{date}`, `{time}`, `{result}` (quello che ha scritto il comando precedente) e `{ask:…}`, che chiede un testo quando l'azione parte. Entrano in modo sicuro: codificati negli indirizzi web, mai trasformati in codice nei comandi.
- Ogni azione può avere una sua scorciatoia da tastiera e partire senza aprire la ruota.
- `wheelor.exe --run <id>` esegue un'azione da uno Stream Deck, da un collegamento o da uno script.
- Le azioni partono una alla volta, ed **Esc** ferma quella in corso (e quelle in attesa). Se un passo non riesce, la notifica dice quale e perché.
- **Prova azione** aspetta 3 secondi quando l'azione lavora sull'app in primo piano, così fai in tempo a passarci.

**Impostazioni**
- La ruota è l'editor: clic su una pallina per vedere cosa fa, doppio clic per modificarla, tasto destro per tutti i comandi, trascinala per spostarla o fuori dal disco per toglierla. Il **+** dopo l'ultima pallina aggiunge un'azione o un sottomenu. Funziona anche la tastiera: frecce, Ctrl+frecce, Canc, Invio, Ins.
- Una nuova finestra **Aggiungi**, con Azioni e Sottomenu, la ricerca, e **Nuova azione** / **Nuovo sottomenu** a portata di mano.
- **Nuova azione** parte da un passo o da uno degli otto esempi pronti: cerca o traduci la selezione, cerca su YouTube, incolla come testo semplice, email con la selezione, scrivi la data di oggi, due app affiancate, pausa.
- I passi si riordinano trascinandoli; i campi di testo hanno il menu **{ } Segnaposto**.
- **Annulla**, anche con Ctrl+Z, per le modifiche a ruote, passi e azioni.
- La finestra si apre a una misura adatta allo schermo, al centro dello schermo con il cursore, con la sua barra del titolo e gli angoli arrotondati di Windows 11.
- Registrare una scorciatoia: premi i tasti insieme e rilasciali; la combinazione è scelta dopo 2 secondi.
- Corretto: registrando una scorciatoia a volte nessun tasto veniva preso, o l'ascolto si interrompeva a metà.
- La pagina Informazioni porta a wheelor.app ed elenca i crediti.
- Testi più chiari ovunque, pensati per chiunque.

**Dietro le quinte**
- I nuovi file delle impostazioni usano id in inglese (`terminal`, `search-selection`…). I file delle versioni precedenti funzionano così come sono.

## 0.2.1-beta.1 — 2026-10-13

**English**
- Sharper Settings window: text and rounded edges are now drawn by Skia, crisp even on 100% screens.

**Italiano**
- Finestra delle impostazioni più nitida: testo e bordi curvi ora li disegna Skia, nitidi anche sugli schermi al 100%.

## 0.2.0-beta.1 — 2026-10-12

**English**
- Shortcuts are recorded by pressing the keys right in the field, with a check that the combination is free and won't take away a key you type with.
- Appearance: accent colour, wheel opacity and size; with "Aim from anywhere" off, a click outside the wheel closes it.
- Update check from wheelor.app, with a notification and one-click update (verified download, silent install).
- English interface, with the language chosen in Settings.
- Microsoft Store version.

**Italiano**
- Le scorciatoie si registrano premendo i tasti direttamente nel campo, controllando che la combinazione sia libera e non tolga un tasto per scrivere.
- Aspetto: colore di accento, opacità e dimensione della ruota; con «Punta da tutto lo schermo» disattivato, un clic fuori dalla ruota la chiude.
- Controllo degli aggiornamenti da wheelor.app, con notifica e aggiornamento in un clic (download verificato, installazione silenziosa).
- Interfaccia in inglese, con la lingua scelta nelle impostazioni.
- Versione per il Microsoft Store.

## 0.1.0-beta.1 — 2026-10-12

**English**
- First public release: a wheel of actions around the cursor, with submenus, wheels for specific apps, animations and a Settings window.

**Italiano**
- Prima versione pubblica: una ruota di azioni attorno al cursore, con sottomenu, ruote per app specifiche, animazioni e la finestra delle impostazioni.
