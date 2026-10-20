# Descarga (clon parcial y sparse) la parte de web-platform-tests que usan los harness:
# html/syntax/parsing y los directorios de reftests de CSS.
# El commit pineado está en tools/wpt-commit.txt.
# Uso (desde cualquier directorio): powershell -File tools/fetch-wpt.ps1
$ErrorActionPreference = 'Stop'

$Root = Split-Path -Parent $PSScriptRoot
$Wpt = Join-Path $Root 'tests/wpt'
$Dirs = @('html/syntax/parsing', 'fonts', 'css/support', 'css/CSS2/box-display', 'css/CSS2/margin-padding-clear', 'css/CSS2/normal-flow', 'css/CSS2/floats', 'css/CSS2/floats-clear', 'css/CSS2/positioning', 'css/CSS2/linebox', 'css/CSS2/text', 'css/CSS2/colors', 'css/CSS2/borders', 'css/CSS2/support', 'css/CSS2/reference', 'css/css-backgrounds', 'css/selectors', 'css/reference', 'url/resources', 'css/css-flexbox')
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
