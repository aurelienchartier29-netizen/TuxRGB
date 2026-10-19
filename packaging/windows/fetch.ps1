# TuxRGB – Windows Beta Build installer helper.
#
# Downloads, at install time, the parts TuxRGB does not ship itself:
#   -What openrgb : the official OpenRGB 1.1 build for Windows (64-bit zip),
#                   unpacked into -Dest (TuxRGB starts it as its private
#                   SDK server, like the copy embedded in the Linux AppImage);
#   -What pawnio  : the official PawnIO driver setup (SMBus access for the
#                   motherboard and the RAM), saved as -Dest\PawnIO_setup.exe.
#
# Exit codes: 0 ok, 2 download failed, 3 unexpected archive content.

param(
    [Parameter(Mandatory = $true)][ValidateSet('openrgb', 'pawnio')][string]$What,
    [Parameter(Mandatory = $true)][string]$Dest,
    [string]$Tag = 'release_1.1',
    [string]$Version = '1.1',
    [string]$Commit = '8e16de2'
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # Invoke-WebRequest is very slow otherwise
try {
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
} catch { }
$Headers = @{ 'User-Agent' = 'TuxRGB-Installer' }

function Get-File([string[]]$Urls, [string]$OutFile, [long]$MinBytes) {
    foreach ($u in $Urls) {
        try {
            Write-Host "Téléchargement : $u"
            Invoke-WebRequest -UseBasicParsing -Uri $u -OutFile $OutFile -Headers $Headers
            if ((Test-Path $OutFile) -and ((Get-Item $OutFile).Length -ge $MinBytes)) {
                return $true
            }
        } catch {
            Write-Host "  échec : $($_.Exception.Message)"
        }
    }
    return $false
}

$tmp = Join-Path $env:TEMP ("tuxrgb-setup-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    if ($What -eq 'pawnio') {
        New-Item -ItemType Directory -Force -Path $Dest | Out-Null
        $out = Join-Path $Dest 'PawnIO_setup.exe'
        $urls = @('https://github.com/namazso/PawnIO.Setup/releases/latest/download/PawnIO_setup.exe')
        if (-not (Get-File $urls $out 100KB)) { exit 2 }
        exit 0
    }

    # OpenRGB: the release assets are listed by the Codeberg API; the usual
    # file names are tried if the API cannot be reached.
    $urls = @()
    try {
        $rel = Invoke-RestMethod -UseBasicParsing -Headers $Headers `
            -Uri "https://codeberg.org/api/v1/repos/OpenRGB/OpenRGB/releases/tags/$Tag"
        $urls += $rel.assets |
            Where-Object { $_.name -match 'Windows' -and $_.name -match '64' -and $_.name -like '*.zip' } |
            ForEach-Object { $_.browser_download_url }
    } catch {
        Write-Host "API Codeberg indisponible : $($_.Exception.Message)"
    }
    $file = "OpenRGB_${Version}_Windows_64_${Commit}.zip"
    $urls += "https://codeberg.org/OpenRGB/OpenRGB/releases/download/$Tag/$file"
    $urls += "https://openrgb.org/releases/$Tag/$file"

    $zip = Join-Path $tmp 'openrgb.zip'
    if (-not (Get-File $urls $zip 1MB)) { exit 2 }

    $x = Join-Path $tmp 'x'
    Expand-Archive -Path $zip -DestinationPath $x -Force
    $exe = Get-ChildItem -Path $x -Recurse -Filter 'OpenRGB.exe' | Select-Object -First 1
    if (-not $exe) { exit 3 }
    if (Test-Path $Dest) { Remove-Item -Recurse -Force $Dest }
    New-Item -ItemType Directory -Force -Path $Dest | Out-Null
    Copy-Item -Path (Join-Path $exe.DirectoryName '*') -Destination $Dest -Recurse -Force
    Write-Host "OpenRGB installé dans $Dest"
    exit 0
} finally {
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmp
}
