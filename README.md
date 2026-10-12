# Wheelor

Azioni rapide su una ruota attorno al cursore, per Windows.

- Sito: https://wheelor.app
- Ultima versione: https://wheelor.app/latest.json
- Download: https://github.com/xGariko/wheelor/releases/latest

```
wheelor/
├── app/                  app nativa (Rust): tutto ciò che serve per compilare exe e installer
│   ├── src/  ui/  assets/  tools/
│   ├── installer/        Inno Setup + build.ps1 → app/dist/
│   └── DESIGN.md         architettura e decisioni dell'app
├── web/                  landing page (HTML + CSS statici, nessuna dipendenza)
│   ├── src/              modello della pagina con i segnaposto {{VERSION}}, {{SIZE}}, …
│   └── build.ps1         riempie i segnaposto da latest.json → web/dist/
└── .github/workflows/
    ├── release.yml       tag vX.Y.Z → compila l'installer → GitHub Release → aggiorna il sito
    └── site.yml          crea il sito e lo pubblica su GitHub Pages
```

## App

```powershell
cd app
cargo run                                                         # prova
powershell -ExecutionPolicy Bypass -File installer\build.ps1      # installer in app\dist
```

`installer\build.ps1` scrive in `app\dist` l'installer, il suo `.sha256` e `latest.json`.

## Sito

```powershell
powershell -ExecutionPolicy Bypass -File web\build.ps1            # usa app\dist\latest.json
python -m http.server 4321 --directory web\dist                   # http://localhost:4321
```

In locale il pulsante scarica l'installer copiato in `web\dist`; online punta alla GitHub Release.

## Dove stanno installer e dati di versione

Non serve un database: i dati sono pochi, cambiano solo a ogni release e sono pubblici.

| Cosa | Dove |
|---|---|
| Installer e `.sha256` | **GitHub Releases**: gratuito, CDN, nessun limite di banda per i repository pubblici, storico di tutte le versioni e conteggio dei download |
| Versione, data, peso, hash, link | `latest.json`, allegato a ogni release e copiato nel sito (`/latest.json`) |
| Landing page | **GitHub Pages** su `wheelor.app`, ricreata da `site.yml` |

La pagina è statica: versione e peso sono scritti nell'HTML al momento della pubblicazione, quindi si vede tutto anche senza JavaScript e nessuna richiesta parte dal browser verso altri servizi (a parte Google Fonts).

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

Lo stesso indirizzo (`https://wheelor.app/latest.json`) potrà servire all'app per controllare gli aggiornamenti.

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

1. aggiorna `version` in `app/Cargo.toml` e fai commit;
2. `git tag v0.2.0` e `git push origin main v0.2.0`.

`release.yml` controlla che tag e versione coincidano, compila e testa su Windows, crea la release e fa ripartire `site.yml`. Dopo qualche minuto il sito mostra la nuova versione. Un tag con trattino (`v0.2.0-beta.1`) crea una pre-release e il sito resta com'è.

Le modifiche a `web/` sul ramo `main` ripubblicano il sito da sole, con i dati dell'ultima release.

## Licenza

Tutti i diritti riservati: il codice è visibile ma non riutilizzabile. Vedi [LICENSE](LICENSE).
