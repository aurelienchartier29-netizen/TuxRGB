# Builds TuxRGB for Windows on Windows 10 / 11:
#   target\release\tuxrgb.exe
#   dist\TuxRGB-<version>-Windows-Setup.exe    (NSIS installer)
#
# Needs:
#   - Rust (https://rustup.rs, MSVC toolchain with the "Desktop development
#     with C++" build tools, or the GNU toolchain);
#   - NSIS 3 (installed automatically with winget if missing).
# The icon and version information are linked when rc.exe (Windows SDK) is
# found; otherwise tuxrgb.exe is built without them (the installer
# shortcuts still use tuxrgb.ico).
#
# Usage (PowerShell, from the project folder):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\build-windows.ps1 [-NoInstaller]

param([switch]$NoInstaller)

$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $Root

$base = (Select-String -Path 'Cargo.toml' -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
$Version = "$base"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Host "Rust n'est pas installé : https://rustup.rs (puis relancez ce script)." -ForegroundColor Red
    Start-Process 'https://rustup.rs'
    exit 1
}

Write-Host "==> Compilation de tuxrgb.exe ($Version)" -ForegroundColor Cyan
cargo build --release --locked -p tuxrgb
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$exe = Join-Path $Root 'target\release\tuxrgb.exe'
if (-not (Test-Path $exe)) { throw "$exe introuvable" }
if ($NoInstaller) { Write-Host $exe; exit 0 }

Write-Host "==> Préparation de l'installateur" -ForegroundColor Cyan
$stage = Join-Path $Root 'build\windows\stage'
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'licenses'), (Join-Path $Root 'dist') | Out-Null
Copy-Item $exe (Join-Path $stage 'tuxrgb.exe')
Copy-Item 'packaging\windows\tuxrgb.ico', 'packaging\windows\installer-side.bmp', 'packaging\windows\fetch.ps1' $stage
Copy-Item 'LICENSE' (Join-Path $stage 'LICENSE.txt')
Copy-Item 'packaging\windows\LIRE-MOI-Windows.txt' $stage
Copy-Item 'licenses\*' (Join-Path $stage 'licenses')

$makensis = Get-Command makensis -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1
if (-not $makensis) {
    foreach ($p in @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe")) {
        if (Test-Path $p) { $makensis = $p; break }
    }
}
if (-not $makensis) {
    Write-Host "NSIS introuvable : installation avec winget…"
    winget install --id NSIS.NSIS -e --accept-package-agreements --accept-source-agreements
    $makensis = "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
}

$setup = Join-Path $Root "dist\TuxRGB-$Version-Windows-Setup.exe"
Write-Host "==> Installateur $setup" -ForegroundColor Cyan
& $makensis /V2 "/DVERSION=$Version" "/DVERSION_NUM=$base.0" "/DSTAGE=$stage" "/DOUTFILE=$setup" 'packaging\windows\tuxrgb.nsi'
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$hash = (Get-FileHash -Algorithm SHA256 $setup).Hash.ToLower()
"$hash  TuxRGB-$Version-Windows-Setup.exe" | Set-Content -Encoding ascii "$setup.sha256"
Write-Host "Terminé : $setup" -ForegroundColor Green
