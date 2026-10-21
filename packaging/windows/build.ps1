# Arma los paquetes de Windows de Freedom en dist\, desde la raíz del repositorio:
#
#   powershell -File packaging\windows\build.ps1 [-Version 0.1.0]
#
# - Freedom-<versión>-windows-x64-setup.exe: el instalador (Inno Setup 6, `iscc`).
# - Freedom-<versión>-windows-x64-portable.zip: el ejecutable suelto, sin instalar.
#
# Compila con `--features gui`: una aplicación de ventana, sin consola detrás.
param([string]$Version = "")

$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..\..")

if (-not $Version) {
    $Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.*)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
}

cargo build --release -p bf-shell --features gui
if ($LASTEXITCODE -ne 0) { throw "cargo build falló" }

New-Item -ItemType Directory -Force dist | Out-Null

# Portable: el ejecutable, el icono y una nota.
$name = "Freedom-$Version-windows-x64-portable"
$stage = Join-Path dist $name
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\freedom.exe $stage
Copy-Item packaging\icons\freedom.ico $stage
@"
Freedom $Version para Windows (64 bits), versión portable

Abre freedom.exe: no hace falta instalar nada. Para tener Freedom en el menú Inicio, usa el
instalador (Freedom-$Version-windows-x64-setup.exe).

Más información: https://github.com/ramirezjhulian7/browser-freedom
"@ | Out-File -Encoding utf8 (Join-Path $stage "LEEME.txt")
$zip = Join-Path dist "$name.zip"
if (Test-Path $zip) { Remove-Item -Force $zip }
Compress-Archive -Path $stage -DestinationPath $zip
Remove-Item -Recurse -Force $stage

# Instalador.
$iscc = Get-Command iscc -ErrorAction SilentlyContinue
if (-not $iscc) {
    $default = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
    if (Test-Path $default) { $iscc = $default } else { throw "Falta Inno Setup 6 (iscc)" }
} else {
    $iscc = $iscc.Source
}
& $iscc "/DAppVersion=$Version" packaging\windows\freedom.iss
if ($LASTEXITCODE -ne 0) { throw "iscc falló" }

Get-ChildItem dist
