<#
.SYNOPSIS
  Crea il pacchetto per il Microsoft Store: dist\Wheelor-<versione>.msix

.DESCRIPTION
  1. compila Wheelor in release (salvo -SkipBuild);
  2. prepara la cartella del pacchetto in target\msix: wheelor.exe, le icone (assets\msix, generate da
     tools\make-icon) e AppxManifest.xml con la versione di Cargo.toml e l'identità di identity.json;
  3. crea resources.pri (Windows sceglie l'icona giusta per ogni scala dello schermo) con makepri;
  4. impacchetta con makeappx.
  Il pacchetto NON è firmato, ed è giusto così: lo firma il Microsoft Store quando lo carichi in
  Partner Center. Richiede il Windows SDK (makeappx.exe e makepri.exe; c'è già sui runner di GitHub).

  Versione: lo Store vuole quattro numeri e l'ultimo a 0. Da X.Y.Z-qualcosa.N si ottiene
  X.Y.(Z*100+N).0 e da X.Y.Z si ottiene X.Y.(Z*100+99).0, così ogni versione è maggiore della
  precedente (0.2.0-beta.1 → 0.2.1.0, 0.2.0-beta.2 → 0.2.2.0, 0.2.0 → 0.2.99.0, 0.2.1 → 0.2.199.0).

.PARAMETER SkipBuild
  Usa target\release\wheelor.exe già compilato (per esempio da installer\build.ps1).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File installer\msix\build.ps1
#>
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

$match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $match) { throw 'Versione non trovata in Cargo.toml' }
$version = $match.Matches[0].Groups[1].Value

function ConvertTo-StoreVersion([string]$Version) {
    if ($Version -notmatch '^(\d+)\.(\d+)\.(\d+)(?:-[0-9A-Za-z]+\.(\d+))?$') {
        throw "Versione $Version non convertibile per lo Store: usa X.Y.Z oppure X.Y.Z-beta.N"
    }
    $pre = if ($Matches[4]) { [int]$Matches[4] } else { 99 }
    if ($pre -gt 98 -and $Matches[4]) { throw "Pre-release troppo alta per lo Store: $Version (massimo .98)" }
    "$($Matches[1]).$($Matches[2]).$([int]$Matches[3] * 100 + $pre).0"
}
$storeVersion = ConvertTo-StoreVersion $version

# Identità del pacchetto: Partner Center > l'app > Gestione prodotto > Identità del prodotto.
$identity = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'identity.json') | ConvertFrom-Json
if (($identity.name, $identity.publisher, $identity.publisherDisplayName) -match 'DA-COMPILARE') {
    throw 'Compila installer\msix\identity.json con i valori di Partner Center (Identità del prodotto): Package/Identity/Name, Package/Identity/Publisher e Package/Properties/PublisherDisplayName'
}

# Gli strumenti del Windows SDK, dalla versione più recente installata.
function Find-SdkTool([string]$Name) {
    $tool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\$Name" -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1
    if (-not $tool) { throw "$Name non trovato: installa il Windows SDK (winget install Microsoft.WindowsSDK.10.0.26100)" }
    $tool.FullName
}
$makeappx = Find-SdkTool 'makeappx.exe'
$makepri = Find-SdkTool 'makepri.exe'

function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE) { throw "$Name non riuscito (codice $LASTEXITCODE)" }
}

Push-Location $root
try {
    if (-not $SkipBuild) { Invoke-Step "Compilazione di Wheelor $version" { cargo build --release --quiet } }

    $layout = Join-Path $root 'target\msix'
    if (Test-Path $layout) { Remove-Item -Recurse -Force $layout }
    New-Item -ItemType Directory -Force (Join-Path $layout 'Assets') | Out-Null
    Copy-Item (Join-Path $root 'target\release\wheelor.exe') $layout
    Copy-Item (Join-Path $root 'assets\msix\*.png') (Join-Path $layout 'Assets')
    # Testi tradotti del manifest: makepri li compila in resources.pri, poi non servono più.
    Copy-Item -Recurse (Join-Path $PSScriptRoot 'Strings') $layout

    $escape = { param($text) [Security.SecurityElement]::Escape($text) }
    $manifest = (Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'AppxManifest.xml')).
        Replace('{{NAME}}', (& $escape $identity.name)).
        Replace('{{PUBLISHER}}', (& $escape $identity.publisher)).
        Replace('{{PUBLISHER_DISPLAY_NAME}}', (& $escape $identity.publisherDisplayName)).
        Replace('{{VERSION}}', $storeVersion)
    [IO.File]::WriteAllText((Join-Path $layout 'AppxManifest.xml'), $manifest, (New-Object Text.UTF8Encoding $false))

    $priconfig = Join-Path $root 'target\msix-priconfig.xml'
    Invoke-Step 'Risorse (resources.pri)' {
        & $makepri createconfig /cf $priconfig /dq en-US /pv 10.0.0 /o | Out-Null
        if ($LASTEXITCODE) { return }
        # Un solo pacchetto con tutte le scale: niente pacchetti di risorse separati (servono ai bundle).
        $config = [xml](Get-Content -Raw $priconfig)
        $config.resources.RemoveChild($config.resources.packaging) | Out-Null
        $config.Save($priconfig)
        & $makepri new /pr $layout /cf $priconfig /mn (Join-Path $layout 'AppxManifest.xml') /of (Join-Path $layout 'resources.pri') /o | Out-Null
    }
    Remove-Item -Recurse -Force (Join-Path $layout 'Strings')

    $dist = Join-Path $root 'dist'
    New-Item -ItemType Directory -Force $dist | Out-Null
    $package = Join-Path $dist "Wheelor-$version.msix"
    Invoke-Step "Pacchetto $storeVersion" { & $makeappx pack /d $layout /p $package /o | Out-Null }
}
finally {
    Pop-Location
}

$size = [math]::Round((Get-Item $package).Length / 1MB, 1)
Write-Host "Creato dist\Wheelor-$version.msix ($size MB, versione per lo Store $storeVersion)" -ForegroundColor Green
Write-Host 'Caricalo in Partner Center (Pacchetti): lo firma il Microsoft Store.'
