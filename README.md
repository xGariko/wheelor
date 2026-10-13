# Wheelor

Azioni rapide su una ruota attorno al cursore, per Windows.

- Sito: https://wheelor.app (in italiano: https://wheelor.app/it/)
- Informativa sulla privacy: https://wheelor.app/privacy.html
- Ultima versione: https://wheelor.app/latest.json
- Download: https://github.com/xGariko/wheelor/releases

```
wheelor/
├── app/                  app nativa (Rust): tutto ciò che serve per compilare exe e installer
│   ├── src/  ui/  assets/  tools/
│   ├── installer/        Inno Setup + build.ps1 → app/dist/
│   └── DESIGN.md         architettura e decisioni dell'app
├── CHANGELOG.md          novità di ogni versione (diventano le note della release)
├── web/                  sito (HTML + CSS statici, nessuna dipendenza), in inglese e in italiano
│   ├── src/              modelli delle pagine (index, privacy), partials/ comuni, caratteri
│   └── build.ps1         crea le pagine nelle due lingue e riempie i segnaposto → web/dist/
└── .github/workflows/
    ├── release.yml       tag vX.Y.Z[-beta.N] → compila l'installer → GitHub Release → aggiorna il sito
    └── site.yml          crea il sito e lo pubblica su GitHub Pages
```

## App

```powershell
cd app
cargo run                                                         # prova
powershell -ExecutionPolicy Bypass -File installer\build.ps1      # installer in app\dist
```

`installer\build.ps1` scrive in `app\dist` l'installer, il suo `.sha256` e `latest.json`.

### Skia

La finestra delle impostazioni usa il renderer Skia di Slint. I binari precompilati di Skia esistono solo con il runtime C dinamico, mentre noi linkiamo quello statico (`app\.cargo\config.toml`, così l'exe non chiede i Visual C++ Redistributable): Skia si compila quindi da sorgente con `clang-cl` e Ninja. Ci vogliono pochi minuti, poi resta in `app\target` finché non cambia la versione di Slint.

Sui runner `windows-latest` di GitHub va tutto da solo (LLVM c'è già, Ninja lo installa `release.yml`). In locale lo scaricamento automatico dei sorgenti si inceppa (collegamenti simbolici, percorsi oltre i 260 caratteri, spazi nel nome della cartella utente), quindi una volta per macchina:

```powershell
winget install LLVM.LLVM Ninja-build.Ninja
git clone --depth 1 --branch m153-0.101.2 https://github.com/rust-skia/skia C:\progetti\skia
cd C:\progetti\skia; $env:GIT_SYNC_DEPS_SKIP_EMSDK = 1; python tools\git-sync-deps
[Environment]::SetEnvironmentVariable('SKIA_SOURCE_DIR', 'C:\progetti\skia', 'User')   # poi riapri il terminale
```

La cartella deve avere un percorso corto e senza spazi. Il tag (`m153-0.101.2`) è la voce `skia` di `[package.metadata]` nel `Cargo.toml` di `skia-bindings`: quando un aggiornamento di Slint porta un'altra versione di `skia-bindings`, va ripetuto con il tag nuovo. Gli errori "Filename too long" di `git-sync-deps` riguardano file di test di dipendenze e si possono ignorare.

## Sito

```powershell
powershell -ExecutionPolicy Bypass -File web\build.ps1            # usa app\dist\latest.json
python -m http.server 4321 --directory web\dist                   # http://localhost:4321
```

Il pulsante principale porta alla scheda del Microsoft Store (l'ID dello Store è in `web\build.ps1`); l'installer è un collegamento secondario in fondo alla pagina: in locale scarica quello copiato in `web\dist`, online punta alla GitHub Release.

Ogni pagina di `web\src` è un modello con i testi nelle due lingue, `{{en}}…{{/en}}{{it}}…{{/it}}`: `build.ps1` ne ricava la versione inglese nella radice e quella italiana in `it/` (la sintassi completa è in cima allo script). Il selettore EN/IT in alto porta alla stessa pagina nell'altra lingua e ricorda la scelta; alla prima visita, chi ha il browser in italiano finisce da solo sulla versione italiana.

## Dove stanno installer e dati di versione

Non serve un database: i dati sono pochi, cambiano solo a ogni release e sono pubblici.

| Cosa | Dove |
|---|---|
| Installer e `.sha256` | **GitHub Releases**: gratuito, CDN, nessun limite di banda per i repository pubblici, storico di tutte le versioni e conteggio dei download |
| Versione, data, peso, hash, link | `latest.json`, allegato a ogni release e copiato nel sito (`/latest.json`) |
| Landing page | **GitHub Pages** su `wheelor.app`, ricreata da `site.yml` |

La pagina è statica: versione e peso sono scritti nell'HTML al momento della pubblicazione, quindi si vede tutto anche senza JavaScript e nessuna richiesta parte dal browser verso altri servizi: anche i caratteri sono serviti dal sito (`web/src/fonts`), niente cookie né statistiche. Se cambia qualcosa (un servizio esterno, una connessione dell'app), va aggiornata anche `web/src/privacy.html`.

`latest.json`:

```json
{
  "version": "0.1.0",
  "date": "2026-10-12",
  "installer": {
    "file": "Wheelor-Setup-0.1.0.exe",
    "url": "https://github.com/xGariko/wheelor/releases/download/v0.1.0/Wheelor-Setup-0.1.0.exe",
    "size": 6529953,
    "sha256": "…"
  }
}
```

Lo stesso indirizzo (`https://wheelor.app/latest.json`) lo legge l'app per controllare gli aggiornamenti (vedi sotto).

## Pubblicazione

Il repository è pubblico (le release di un repository privato non si scaricano senza accesso) e GitHub Pages usa come sorgente **GitHub Actions** (*Settings → Pages*), con dominio personalizzato `wheelor.app`.

DNS del dominio (Hostinger):

| Tipo | Nome | Valore |
|---|---|---|
| A | `@` | `185.199.108.153`, `185.199.109.153`, `185.199.110.153`, `185.199.111.153` |
| AAAA | `@` | `2606:50c0:8000::153`, `2606:50c0:8001::153`, `2606:50c0:8002::153`, `2606:50c0:8003::153` |
| CNAME | `www` | `xgariko.github.io` |
| TXT | `_github-pages-challenge-xGariko` | codice da *GitHub → Settings → Pages → Add a domain* (verifica del dominio) |

A ogni versione:

1. aggiorna `version` in `app/Cargo.toml` e aggiungi in cima a `CHANGELOG.md` la sezione `## <versione> — <data>` con le novità, in inglese e in italiano; fai commit;
2. `git tag v0.2.0` e `git push origin main v0.2.0`.

`release.yml` controlla che tag e versione coincidano, compila e testa su Windows, crea la release (con la sezione del changelog come descrizione: è la pagina che apre «Novità» nell'app) e fa ripartire `site.yml`. Dopo qualche minuto il sito mostra la nuova versione. Un tag con trattino (`v0.2.0-beta.1`) crea una pre-release: finché non esiste una versione stabile il sito mostra quella, con l'etichetta **Beta**; dopo, il sito resta sull'ultima stabile.

Le modifiche a `web/` sul ramo `main` ripubblicano il sito da sole, con i dati dell'ultima release.

### Come arrivano gli aggiornamenti agli utenti

Non serve fare altro che pubblicare la release: quando `site.yml` ha aggiornato `https://wheelor.app/latest.json`, ogni Wheelor installato (dalla prima versione dopo la 0.1.0-beta.1) lo legge entro 20 secondi dall'avvio e poi ogni 6 ore. Se la versione è più nuova:

1. compare una notifica *"Wheelor X è disponibile — Clic qui per aggiornare"* e nel menu dell'icona la voce *Aggiorna a Wheelor X…*; nelle impostazioni, un riquadro con **Aggiorna**;
2. **Aggiorna** scarica l'installer dalla GitHub Release, controlla lo SHA-256 scritto in `latest.json` e lo avvia in modalità silenziosa;
3. l'installer chiude Wheelor, sostituisce l'exe e lo riavvia: una notifica conferma la nuova versione. Ruote, azioni e preferenze restano in `%APPDATA%\Wheelor`.

Chi ha ancora la 0.1.0-beta.1 (senza controllo degli aggiornamenti) deve installare la nuova a mano una volta, dal sito: l'installer aggiorna sopra quella esistente.

Le pre-release contano come aggiornamenti per chi ha una versione più vecchia (`0.2.0-beta.1` > `0.1.0`), ma solo finché il sito le mostra: dopo la prima versione stabile `latest.json` punta sempre all'ultima stabile. Il controllo si può disattivare in *Generale → Sistema → Cerca aggiornamenti*; non invia dati, è una semplice lettura di `latest.json`.

## Microsoft Store

Lo Store firma l'app gratis: chi la installa da lì non vede l'avviso "file non sicuro" del browser e riceve gli aggiornamenti dallo Store. È lo stesso exe, impacchettato in MSIX.

Una volta sola:

1. in [Partner Center](https://partner.microsoft.com/dashboard) prenota il nome *Wheelor*;
2. in *Gestione prodotto → Identità del prodotto* copia `Package/Identity/Name`, `Package/Identity/Publisher` e `Package/Properties/PublisherDisplayName` in `app/installer/msix/identity.json` e fai commit.

A ogni versione (dopo la release):

1. scarica l'artefatto **Wheelor-msix** dall'esecuzione di `release.yml` (*Actions → Release → l'esecuzione → Artifacts*), oppure crealo in locale con `powershell -ExecutionPolicy Bypass -File app\installer\msix\build.ps1`;
2. in Partner Center crea un nuovo invio, carica il `.msix` in *Pacchetti* e, se l'aspetto è cambiato, aggiorna gli screenshot della scheda (in ogni lingua): `powershell -ExecutionPolicy Bypass -File app\installer\msix\screenshots.ps1` li crea in `app\dist\store\en` e `app\dist\store\it`, numerati nell'ordine in cui caricarli; poi invia. La certificazione richiede da qualche ora a qualche giorno, poi lo Store aggiorna gli utenti da solo.

Differenze rispetto all'installer: niente aggiornamenti dall'app (li fa lo Store), avvio con Windows gestito anche da *Impostazioni → App → Avvio*, impostazioni in una cartella privata dell'app (le apre il pulsante *Apri cartella*), Windows 10 2004 o successivo. Meglio non installare entrambe le versioni: ne gira comunque una sola alla volta.

## Licenza

Tutti i diritti riservati: il codice è visibile ma non riutilizzabile. Vedi [LICENSE](LICENSE).
