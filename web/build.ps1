<#
.SYNOPSIS
  Crea il sito in web\dist a partire da web\src.

.DESCRIPTION
  Il sito è in inglese (nella radice) e in italiano (nella cartella it/). Ogni pagina .html di
  web\src è un modello da cui escono le due versioni, con questa sintassi:
  - {{en}}…{{/en}} e {{it}}…{{/it}}: testo di una lingua sola (l'altro viene tolto);
  - {{>nome}}: il contenuto di web\src\partials\nome.html (inizio di <head>, intestazione, piè di pagina);
  - {{VERSION}}, {{SIZE}}, {{DOWNLOAD_URL}}, {{BETA_TAG}}: dati di latest.json, che scrive
    app\installer\build.ps1 e che viene allegato a ogni release;
  - {{LANG}} (en o it), {{ROOT}} (percorso della radice del sito: vuoto o ../), {{HOME}} (la pagina
    principale: vuoto se è questa), {{SWITCH}} (la stessa pagina nell'altra lingua), {{YEAR}}
    e {{HEAD_META}} (tag per motori di ricerca e anteprime): li calcola questo script.
  I commenti HTML dei modelli sono note per chi li modifica e non finiscono nel sito.
  Gli altri file (stile, logo, caratteri) vengono copiati così come sono, e nel sito finisce anche
  latest.json, così l'ultima versione si legge sempre allo stesso indirizzo.
  Funziona con Windows PowerShell 5.1 e con PowerShell 7 (anche su Linux, nella GitHub Action).

.PARAMETER Release
  Il latest.json da usare. Predefinito: app\dist\latest.json, cioè l'ultimo installer compilato
  in locale; in quel caso l'installer viene copiato nel sito, così il pulsante funziona anche in prova.

.PARAMETER SiteUrl
  Indirizzo pubblico del sito, es. https://wheelor.app/ (facoltativo): serve per gli indirizzi
  canonici, per i collegamenti tra le due lingue e per l'immagine delle anteprime sui social.

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

# La prima è la lingua della radice del sito e quella per chi non parla nessuna delle due.
$languages = @(
    [pscustomobject]@{ Code = 'en'; Dir = '';    Locale = 'en_US'; Decimal = '.' }
    [pscustomobject]@{ Code = 'it'; Dir = 'it/'; Locale = 'it_IT'; Decimal = ',' }
)

if (-not (Test-Path $Release)) {
    throw "Non trovo ${Release}: compila prima l'installer (app\installer\build.ps1) oppure indica -Release"
}
$Release = (Resolve-Path $Release).Path
$info = Get-Content -Raw -Encoding UTF8 $Release | ConvertFrom-Json
$version = $info.version
$installer = $info.installer
# Formato invariante e poi il separatore decimale di ogni lingua: non dipende dal Windows di chi compila.
$size = [string]::Format([Globalization.CultureInfo]::InvariantCulture, '{0:0.0} MB', $installer.size / 1MB)
$isPublic = $installer.url -match '^https?://'
# Versione con trattino (0.2.0-beta.1): pre-release, la pagina mostra l'etichetta Beta.
$isBeta = $version.Contains('-')

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
Copy-Item -Recurse $src $out
# Modelli e parti comuni non vanno pubblicati così come sono: le pagine vere le scrive il ciclo in fondo.
Remove-Item -Recurse -Force (Join-Path $out 'partials')
Get-ChildItem $out -Filter *.html -File | Remove-Item
Copy-Item $Release (Join-Path $out 'latest.json')
if (-not $isPublic) {
    # Prova in locale: l'installer sta nella radice del sito, il link è relativo.
    $exe = Join-Path (Split-Path -Parent $Release) $installer.file
    if (Test-Path $exe) { Copy-Item $exe $out } else { Write-Warning "Installer non trovato: $exe" }
}

$partials = @{}
Get-ChildItem (Join-Path $src 'partials') -Filter *.html | ForEach-Object {
    $partials[$_.BaseName] = [IO.File]::ReadAllText($_.FullName, $utf8)
}

function Html([string]$Text) { [Net.WebUtility]::HtmlEncode($Text) }

function Add-Partials([string]$Page) {
    [regex]::Replace($Page, '\{\{>(\w+)\}\}', {
        param($m)
        $name = $m.Groups[1].Value
        if (-not $partials.ContainsKey($name)) { throw "Parte comune sconosciuta: partials\$name.html" }
        $partials[$name]
    })
}

# Tiene il testo della lingua scelta e toglie quello delle altre.
function Select-Language([string]$Page, [string]$Code) {
    $pattern = '\{\{(' + ($languages.Code -join '|') + ')\}\}([\s\S]*?)\{\{/\1\}\}'
    [regex]::Replace($Page, $pattern, {
        param($m)
        if ($m.Groups[1].Value -eq $Code) { $m.Groups[2].Value } else { '' }
    }.GetNewClosure())
}

# Tag per motori di ricerca e anteprime dei link; titolo e descrizione arrivano dalla pagina stessa.
# $Path è la pagina relativa alla radice di una lingua: vuoto per la principale, altrimenti es. privacy.html.
function Get-HeadMeta([string]$Page, $Language, [string]$Path) {
    $title = [regex]::Match($Page, '<title>(.*?)</title>').Groups[1].Value
    $description = [regex]::Match($Page, '<meta name="description" content="([^"]*)"').Groups[1].Value
    $meta = @(
        '<meta property="og:type" content="website">'
        '<meta property="og:site_name" content="Wheelor">'
        "<meta property=""og:title"" content=""$title"">"
        "<meta property=""og:description"" content=""$description"">"
        "<meta property=""og:locale"" content=""$($Language.Locale)"">"
    )
    foreach ($other in $languages | Where-Object Code -ne $Language.Code) {
        $meta += "<meta property=""og:locale:alternate"" content=""$($other.Locale)"">"
    }
    $meta += '<meta name="twitter:card" content="summary">'
    if ($SiteUrl) {
        $base = $SiteUrl.TrimEnd('/') + '/'
        $url = $base + $Language.Dir + $Path
        $meta += "<link rel=""canonical"" href=""$(Html $url)"">"
        foreach ($l in $languages) {
            $meta += "<link rel=""alternate"" hreflang=""$($l.Code)"" href=""$(Html ($base + $l.Dir + $Path))"">"
        }
        $meta += "<link rel=""alternate"" hreflang=""x-default"" href=""$(Html ($base + $languages[0].Dir + $Path))"">"
        $meta += "<meta property=""og:url"" content=""$(Html $url)"">"
        $meta += "<meta property=""og:image"" content=""$(Html "${base}logo-256.png")"">"
    }
    if (-not $Path) {
        # Scheda dell'app per i motori di ricerca, solo sulla pagina principale.
        $app = [ordered]@{
            '@context'          = 'https://schema.org'
            '@type'             = 'SoftwareApplication'
            name                = 'Wheelor'
            description         = [Net.WebUtility]::HtmlDecode($description)
            inLanguage          = $Language.Code
            applicationCategory = 'UtilitiesApplication'
            operatingSystem     = 'Windows 10, Windows 11'
            softwareVersion     = $version
            fileSize            = $size
            offers              = [ordered]@{ '@type' = 'Offer'; price = '0'; priceCurrency = 'EUR' }
        }
        if ($isPublic) { $app.downloadUrl = $installer.url }
        $json = ($app | ConvertTo-Json -Compress).Replace('</', '<\/')
        $meta += "<script type=""application/ld+json"">$json</script>"
    }
    $meta -join "`n"
}

$pages = 0
foreach ($template in Get-ChildItem $src -Filter *.html -File) {
    $source = [IO.File]::ReadAllText($template.FullName, $utf8)
    $source = Add-Partials $source
    $source = [regex]::Replace($source, '<!--[\s\S]*?-->\r?\n?', '')   # note per chi modifica i modelli
    $path = if ($template.Name -eq 'index.html') { '' } else { $template.Name }

    foreach ($language in $languages) {
        $other = $languages | Where-Object Code -ne $language.Code | Select-Object -First 1
        # Quante cartelle risalire per tornare alla radice del sito (it/ → ../).
        $root = '../' * ($language.Dir.Split('/').Count - 1)
        $page = Select-Language $source $language.Code
        $values = [ordered]@{
            '{{LANG}}'         = $language.Code
            '{{ROOT}}'         = $root
            '{{HOME}}'         = if ($path) { './' } else { '' }
            '{{SWITCH}}'       = $root + $other.Dir + $path
            '{{VERSION}}'      = Html $version
            '{{SIZE}}'         = Html $size.Replace('.', $language.Decimal)
            '{{DOWNLOAD_URL}}' = Html $(if ($isPublic) { $installer.url } else { $root + $installer.url })
            '{{BETA_TAG}}'     = if ($isBeta) { '<span class="badge-tag badge-beta">BETA</span>' } else { '' }
            '{{YEAR}}'         = (Get-Date).Year
            '{{HEAD_META}}'    = Get-HeadMeta $page $language $path
        }
        foreach ($key in $values.Keys) { $page = $page.Replace($key, [string]$values[$key]) }
        if ($page -match '\{\{[^}]*\}\}') { throw "Segnaposto sconosciuto in $($template.Name) ($($language.Code)): $($Matches[0])" }

        $target = Join-Path $out ($language.Dir + $template.Name)
        New-Item -ItemType Directory -Force (Split-Path -Parent $target) | Out-Null
        [IO.File]::WriteAllText($target, $page, $utf8)
        $pages++
    }
}

Write-Host "Sito creato in web\dist: Wheelor $version, $size, $pages pagine ($($languages.Code -join ', '))" -ForegroundColor Green
if (-not $isPublic) { Write-Host 'Link di download locale (relativo): va bene per provare, non per pubblicare.' }
