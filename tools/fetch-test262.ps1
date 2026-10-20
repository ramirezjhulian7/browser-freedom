# Descarga (clon parcial y sparse) test262 en tests/test262: harness, test/language,
# test/built-ins, test/annexB y test/intl402. El commit pineado está en tools/test262-commit.txt.
# Uso (desde cualquier directorio): powershell -File tools/fetch-test262.ps1
$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
$Dest = Join-Path $Root 'tests/test262'
$Dirs = @('harness', 'test/language', 'test/built-ins', 'test/annexB', 'test/intl402')
$Commit = (Get-Content (Join-Path $Root 'tools/test262-commit.txt') -Raw).Trim()

function Invoke-Git { & git.exe @args; if ($LASTEXITCODE -ne 0) { throw "git $args falló" } }

if (Test-Path (Join-Path $Dest '.git')) {
    Invoke-Git -C $Dest fetch --filter=blob:none origin $Commit
    Invoke-Git -C $Dest sparse-checkout set @Dirs
} else {
    Invoke-Git -c core.autocrlf=false clone --filter=blob:none --no-checkout https://github.com/tc39/test262 $Dest
    Invoke-Git -C $Dest config core.autocrlf false
    Invoke-Git -C $Dest sparse-checkout init --cone
    Invoke-Git -C $Dest sparse-checkout set @Dirs
}
Invoke-Git -C $Dest checkout --detach $Commit
Write-Output "tests/test262 en $Commit"
