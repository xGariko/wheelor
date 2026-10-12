<#
.SYNOPSIS
  Crea l'installer di Wheelor: dist\Wheelor-Setup-<versione>.exe

.DESCRIPTION
  1. esegue i test e compila Wheelor in release;
  2. compila installer\wheelor.iss con Inno Setup, passando la versione di Cargo.toml;
  3. scrive accanto all'installer il suo hash SHA-256 (serve a winget e a chi lo scarica);
  4. scrive dist\latest.json: versione, data, nome, indirizzo, peso e hash dell'installer.
     Lo usa il sito (web\build.ps1) e viene allegato a ogni release.
  Richiede Rust (cargo) e Inno Setup 6.6 o successivo (winget install JRSoftware.InnoSetup).

.PARAMETER SkipTests
  Non esegue cargo test.

.PARAMETER DownloadBase
  Indirizzo da cui si scaricherà l'installer, senza il nome del file
  (es. https://github.com/utente/wheelor/releases/download/v0.1.0). Lo imposta la release automatica;
  se manca, latest.json contiene solo il nome del file (va bene per provare il sito in locale).

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File installer\build.ps1
#>
param([switch]$SkipTests, [string]$DownloadBase = '')

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
if (-not $match) { throw 'Versione non trovata in Cargo.toml' }
$version = $match.Matches[0].Groups[1].Value

$iscc = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup non trovato: installalo con  winget install JRSoftware.InnoSetup' }

function Invoke-Step([string]$Name, [scriptblock]$Command) {
    Write-Host "==> $Name" -ForegroundColor Cyan
    & $Command
    if ($LASTEXITCODE) { throw "$Name non riuscito (codice $LASTEXITCODE)" }
}

Push-Location $root
try {
    # Un Wheelor avviato da target\release blocca l'exe: va chiuso prima di ricompilare.
    $exe = Join-Path $root 'target\release\wheelor.exe'
    if (Get-Process wheelor -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }) {
        Write-Host '==> Chiudo Wheelor in esecuzione da target\release' -ForegroundColor Cyan
        & $exe --quit
        Get-Process wheelor -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe } | Stop-Process -Force
    }

    if (-not $SkipTests) { Invoke-Step 'Test' { cargo test --quiet } }
    Invoke-Step "Compilazione di Wheelor $version" { cargo build --release --quiet }
    Invoke-Step 'Installer' { & $iscc /Q "/DAppVersion=$version" (Join-Path $PSScriptRoot 'wheelor.iss') }
}
finally {
    Pop-Location
}

$name = "Wheelor-Setup-$version.exe"
$setup = Join-Path $root "dist\$name"
$hash = (Get-FileHash $setup -Algorithm SHA256).Hash
"$hash  $name" | Set-Content -Encoding ascii "$setup.sha256"
$bytes = (Get-Item $setup).Length
$size = [math]::Round($bytes / 1MB, 1)

# Dati della release per il sito (e, in futuro, per il controllo degli aggiornamenti).
$latest = [ordered]@{
    version   = $version
    date      = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
    installer = [ordered]@{
        file   = $name
        url    = if ($DownloadBase) { "$($DownloadBase.TrimEnd('/'))/$name" } else { $name }
        size   = $bytes
        sha256 = $hash.ToLowerInvariant()
    }
}
# UTF-8 senza BOM: Set-Content di Windows PowerShell lo aggiungerebbe.
[IO.File]::WriteAllText((Join-Path $root 'dist\latest.json'), ($latest | ConvertTo-Json), (New-Object Text.UTF8Encoding $false))
Write-Host "Creato dist\Wheelor-Setup-$version.exe ($size MB)" -ForegroundColor Green
Write-Host "SHA-256: $hash"
