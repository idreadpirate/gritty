# uninstall.ps1 - remove a gritty installation made by install.ps1.
# ASCII-only strings: Windows PowerShell 5.1 reads BOM-less files as ANSI, and
# a UTF-8 em-dash decodes to a cp1252 curly quote that terminates the string.
#
#   irm https://raw.githubusercontent.com/idreadpirate/gritty/master/scripts/uninstall.ps1 | iex
#
# Removes the install directory, the Start Menu + Desktop shortcuts, and the
# PATH entry. Leaves your config/session (%APPDATA%\gritty, %LOCALAPPDATA%\gritty)
# untouched unless you pass $env:GRITTY_PURGE = '1'.

$ErrorActionPreference = 'Stop'

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\gritty'

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Done($m) { Write-Host "[ok] $m" -ForegroundColor Green }

Step "Removing shortcuts"
$startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\gritty.lnk'
$desktop   = Join-Path ([Environment]::GetFolderPath('Desktop')) 'gritty.lnk'
foreach ($lnk in @($startMenu, $desktop)) {
    if (Test-Path $lnk) { Remove-Item $lnk -Force }
}

Step "Removing from PATH"
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath) {
    $kept = ($userPath -split ';') | Where-Object { $_ -and $_ -ne $InstallDir }
    [Environment]::SetEnvironmentVariable('Path', ($kept -join ';'), 'User')
}

Step "Removing $InstallDir"
if (Test-Path $InstallDir) {
    try {
        Remove-Item $InstallDir -Recurse -Force
    } catch {
        Write-Host "[warn] could not remove $InstallDir - close any running gritty and retry." -ForegroundColor Yellow
    }
}

if ($env:GRITTY_PURGE -eq '1') {
    Step "Purging config + session"
    foreach ($d in @((Join-Path $env:APPDATA 'gritty'), (Join-Path $env:LOCALAPPDATA 'gritty'))) {
        if (Test-Path $d) { Remove-Item $d -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

Done "gritty uninstalled"
