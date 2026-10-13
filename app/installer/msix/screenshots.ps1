<#
.SYNOPSIS
  Crea gli screenshot per la scheda del Microsoft Store: dist\store\<lingua>\*.png

.DESCRIPTION
  Per l'inglese (en) e l'italiano (it), a 1366×768 (la misura minima dello Store per il desktop):
    1. la ruota aperta su uno sfondo nei colori del sito (test store_screenshot in overlay.rs);
    2. le impostazioni: l'editor della ruota, un'azione a più passi, la finestra "Nuova azione" con
       gli esempi e la pagina Generale (test settings_snapshots con WHEELOR_STORE_SHOTS=1).
  I test scrivono BMP in target\; qui diventano PNG numerati nell'ordine in cui caricarli in
  Partner Center (Scheda dello Store > Screenshot, una scheda per lingua).
  Le impostazioni sono disegnate dal renderer software di Slint, l'unico che funziona senza
  finestra: un po' meno fini della finestra vera (vedi DESIGN.md).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File installer\msix\screenshots.ps1
#>
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
Add-Type -AssemblyName System.Drawing

# Nome scritto dai test → nome del PNG, nell'ordine della scheda.
$shots = [ordered]@{
    'store-ruota-{0}.bmp'                 = '1-ruota.png'
    'settings-store-ruote-{0}.bmp'        = '2-editor-della-ruota.png'
    'settings-store-passi-{0}.bmp'        = '3-azione-a-passi.png'
    'settings-store-nuova-azione-{0}.bmp' = '4-nuova-azione.png'
    'settings-store-generale-{0}.bmp'     = '5-generale.png'
}

function Invoke-Test([string]$Name, [hashtable]$Environment) {
    foreach ($key in $Environment.Keys) { Set-Item "env:$key" $Environment[$key] }
    try {
        cargo test --quiet -- --ignored --exact $Name
        if ($LASTEXITCODE) { throw "cargo test $Name non riuscito (codice $LASTEXITCODE)" }
    }
    finally {
        foreach ($key in $Environment.Keys) { Remove-Item "env:$key" -ErrorAction SilentlyContinue }
    }
}

Push-Location $root
try {
    foreach ($language in 'en', 'it') {
        Write-Host "==> Screenshot in $language" -ForegroundColor Cyan
        Invoke-Test 'overlay::tests::store_screenshot' @{ WHEELOR_LANG = $language }
        Invoke-Test 'settings::tests::settings_snapshots' @{ WHEELOR_LANG = $language; WHEELOR_STORE_SHOTS = '1' }

        $out = Join-Path $root "dist\store\$language"
        New-Item -ItemType Directory -Force $out | Out-Null
        foreach ($shot in $shots.GetEnumerator()) {
            $bmp = Join-Path $root ("target\" + ($shot.Key -f $language))
            $png = Join-Path $out $shot.Value
            $image = [System.Drawing.Image]::FromFile($bmp)
            try { $image.Save($png, [System.Drawing.Imaging.ImageFormat]::Png) } finally { $image.Dispose() }
            Write-Host "    $png"
        }
    }
}
finally {
    Pop-Location
}
