# Descarga (clon parcial y sparse) la parte de web-platform-tests que usan los harness y las
# mediciones: css, html y svg enteros (reftests y parsing), y los directorios de testharness.js
# (resources, common, dom, url, encoding, fetch, xhr, streams, IndexedDB, workers,
# service-workers, websockets, domparsing, quirks, images, webstorage…).
# El commit pineado está en tools/wpt-commit.txt.
# Uso (desde cualquier directorio): powershell -File tools/fetch-wpt.ps1
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Wpt = Join-Path $Root 'tests/wpt'
$Dirs = @('fonts', 'resources', 'common', 'dom', 'url', 'encoding', 'console', 'interfaces', 'shadow-dom', 'custom-elements', 'css', 'html', 'svg', 'fetch', 'xhr', 'streams', 'IndexedDB', 'workers', 'service-workers', 'websockets', 'domparsing', 'quirks', 'images', 'webstorage')
$Commit = (Get-Content (Join-Path $Root 'tools/wpt-commit.txt') -Raw).Trim()

function Invoke-Git {
    & git @args
    if ($LASTEXITCODE -ne 0) { throw "git $args falló con código $LASTEXITCODE" }
}

if (Test-Path (Join-Path $Wpt '.git')) {
    Invoke-Git -C $Wpt fetch --filter=blob:none origin $Commit
    Invoke-Git -C $Wpt sparse-checkout set @Dirs
} else {
    Invoke-Git -c core.autocrlf=false clone --filter=blob:none --no-checkout https://github.com/web-platform-tests/wpt $Wpt
    Invoke-Git -C $Wpt config core.autocrlf false
    Invoke-Git -C $Wpt sparse-checkout init --cone
    Invoke-Git -C $Wpt sparse-checkout set @Dirs
}
Invoke-Git -C $Wpt checkout --detach $Commit
Write-Output "tests/wpt en $Commit"
