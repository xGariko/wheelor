<#
.SYNOPSIS
  Crea la landing page in web\dist a partire da web\src.

.DESCRIPTION
  Copia web\src in web\dist e riempie i segnaposto delle pagine HTML ({{VERSION}}, {{SIZE}},
  {{DOWNLOAD_URL}}, {{YEAR}}, {{HEAD_META}}) con i dati di latest.json, che scrive
  app\installer\build.ps1 e che viene allegato a ogni release. Copia nel sito anche latest.json,
  così l'ultima versione si legge sempre allo stesso indirizzo.
  Funziona con Windows PowerShell 5.1 e con PowerShell 7 (anche su Linux, nella GitHub Action).

.PARAMETER Release
  Il latest.json da usare. Predefinito: app\dist\latest.json, cioè l'ultimo installer compilato
  in locale; in quel caso l'installer viene copiato nel sito, così il pulsante funziona anche in prova.

.PARAMETER SiteUrl
  Indirizzo pubblico del sito, es. https://utente.github.io/wheelor/ (facoltativo):
  serve per l'indirizzo canonico e per l'immagine delle anteprime sui social.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File web\build.ps1
#>
param(
    [string]$Release = (Join-Path $PSScriptRoot '../app/dist/latest.json'),
    [string]$SiteUrl = ''
)

$ErrorActionPreference = 'Stop'
$src = Join-Path $PSScriptRoot 'src'
$out = Join-Path $PSScriptRoot 'dist'
$utf8 = New-Object Text.UTF8Encoding $false

if (-not (Test-Path $Release)) {
    throw "Non trovo ${Release}: compila prima l'installer (app\installer\build.ps1) oppure indica -Release"
}
$Release = (Resolve-Path $Release).Path
$info = Get-Content -Raw -Encoding UTF8 $Release | ConvertFrom-Json
$version = $info.version
$installer = $info.installer
# Punto come separatore decimale anche su un Windows in italiano: la pagina è in inglese.
$size = [string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0:0.0} MB', $installer.size / 1MB)
$isPublic = $installer.url -match '^https?://'

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
Copy-Item -Recurse $src $out
Copy-Item $Release (Join-Path $out 'latest.json')
if (-not $isPublic) {
    # Prova in locale: l'installer sta accanto alla pagina, il link è relativo.
    $exe = Join-Path (Split-Path -Parent $Release) $installer.file
    if (Test-Path $exe) { Copy-Item $exe $out } else { Write-Warning "Installer non trovato: $exe" }
}

function Html([string]$Text) { [Net.WebUtility]::HtmlEncode($Text) }

# Tag per motori di ricerca e anteprime dei link; titolo e descrizione arrivano dalla pagina stessa.
function Get-HeadMeta([string]$Page) {
    $title = [regex]::Match($Page, '<title>(.*?)</title>').Groups[1].Value
    $description = [regex]::Match($Page, '<meta name="description" content="([^"]*)"').Groups[1].Value
    $meta = @(
        '<meta property="og:type" content="website">'
        "<meta property=""og:site_name"" content=""Wheelor"">"
        "<meta property=""og:title"" content=""$title"">"
        "<meta property=""og:description"" content=""$description"">"
        '<meta name="twitter:card" content="summary">'
    )
    if ($SiteUrl) {
        $base = $SiteUrl.TrimEnd('/') + '/'
        $meta += "<link rel=""canonical"" href=""$(Html $base)"">"
        $meta += "<meta property=""og:url"" content=""$(Html $base)"">"
        $meta += "<meta property=""og:image"" content=""$(Html "${base}logo-256.png")"">"
    }
    $app = [ordered]@{
        '@context'          = 'https://schema.org'
        '@type'             = 'SoftwareApplication'
        name                = 'Wheelor'
        description         = [Net.WebUtility]::HtmlDecode($description)
        applicationCategory = 'UtilitiesApplication'
        operatingSystem     = 'Windows 10, Windows 11'
        softwareVersion     = $version
        fileSize            = $size
        offers              = [ordered]@{ '@type' = 'Offer'; price = '0'; priceCurrency = 'EUR' }
    }
    if ($isPublic) { $app.downloadUrl = $installer.url }
    $json = ($app | ConvertTo-Json -Compress).Replace('</', '<\/')
    $meta += "<script type=""application/ld+json"">$json</script>"
    $meta -join "`n"
}

Get-ChildItem $out -Filter *.html -Recurse | ForEach-Object {
    $page = [IO.File]::ReadAllText($_.FullName, $utf8)
    $page = [regex]::Replace($page, '<!--[\s\S]*?-->\r?\n?', '')   # note per chi modifica il modello
    $values = [ordered]@{
        '{{VERSION}}'      = Html $version
        '{{SIZE}}'         = Html $size
        '{{DOWNLOAD_URL}}' = Html $installer.url
        '{{YEAR}}'         = (Get-Date).Year
        '{{HEAD_META}}'    = Get-HeadMeta $page
    }
    foreach ($key in $values.Keys) { $page = $page.Replace($key, [string]$values[$key]) }
    if ($page -match '\{\{\w+\}\}') { throw "Segnaposto sconosciuto in $($_.Name): $($Matches[0])" }
    [IO.File]::WriteAllText($_.FullName, $page, $utf8)
}

Write-Host "Sito creato in web\dist: Wheelor $version, $size" -ForegroundColor Green
if (-not $isPublic) { Write-Host 'Link di download locale (relativo): va bene per provare, non per pubblicare.' }
