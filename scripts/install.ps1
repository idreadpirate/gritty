# install.ps1 - one-line installer for gritty.
#
#   irm https://raw.githubusercontent.com/idreadpirate/gritty/master/scripts/install.ps1 | iex
#
# Downloads the latest released gritty.exe, installs it under
# %LOCALAPPDATA%\Programs\gritty, creates Start Menu + Desktop shortcuts, and
# adds the install directory to the user PATH. No admin rights required.
#
# Optional overrides (set before piping to iex):
#   $env:GRITTY_VERSION = 'v0.1.0'   # install a specific tag instead of latest
#   $env:GRITTY_NO_PATH = '1'        # skip modifying PATH
#   $env:GRITTY_NO_SHORTCUT = '1'    # skip creating shortcuts

$ErrorActionPreference = 'Stop'
# TLS 1.2 for Windows PowerShell 5.1 (older defaults can't reach GitHub).
# pwsh 7 negotiates this on its own; keep the assignment tolerant either way.
try { [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 } catch {}

$Repo       = 'idreadpirate/gritty'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\gritty'
$ExePath    = Join-Path $InstallDir 'gritty.exe'
$Headers    = @{ 'User-Agent' = 'gritty-installer' }

function Info($m) { Write-Host "  $m" -ForegroundColor Gray }
function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Done($m) { Write-Host "[ok] $m" -ForegroundColor Green }
function Die($m)  { Write-Host "[error] $m" -ForegroundColor Red; exit 1 }

Write-Host ""
Write-Host "  gritty installer" -ForegroundColor White
Write-Host ""

# --- 1. Resolve the release to install --------------------------------------
Step "Finding release"
$apiBase = "https://api.github.com/repos/$Repo/releases"
try {
    if ($env:GRITTY_VERSION) {
        $rel = Invoke-RestMethod -Headers $Headers -Uri "$apiBase/tags/$($env:GRITTY_VERSION)"
    } else {
        $rel = Invoke-RestMethod -Headers $Headers -Uri "$apiBase/latest"
    }
} catch {
    Die "Could not query GitHub releases for $Repo. Is the repository public and does it have a release yet? ($_)"
}
$tag = $rel.tag_name
Info "release $tag"

$asset = $rel.assets | Where-Object { $_.name -eq 'gritty.exe' } | Select-Object -First 1
if (-not $asset) { Die "Release $tag has no 'gritty.exe' asset." }
$shaAsset = $rel.assets | Where-Object { $_.name -eq 'gritty.exe.sha256' } | Select-Object -First 1

# --- 2. Download (to a temp file first) -------------------------------------
Step "Downloading gritty.exe"
$tmp = Join-Path ([IO.Path]::GetTempPath()) "gritty-$([Guid]::NewGuid()).exe"
try {
    Invoke-WebRequest -Headers $Headers -Uri $asset.browser_download_url -OutFile $tmp
} catch {
    Die "Download failed: $_"
}

# --- 3. Verify checksum if the release publishes one ------------------------
if ($shaAsset) {
    Step "Verifying checksum"
    $shaTmp = "$tmp.sha256"
    Invoke-WebRequest -Headers $Headers -Uri $shaAsset.browser_download_url -OutFile $shaTmp
    $expected = ((Get-Content $shaTmp -Raw).Trim() -split '\s+')[0].ToLower()
    $actual   = (Get-FileHash -Algorithm SHA256 $tmp).Hash.ToLower()
    Remove-Item $shaTmp -Force -ErrorAction SilentlyContinue
    if ($expected -ne $actual) {
        Remove-Item $tmp -Force -ErrorAction SilentlyContinue
        Die "Checksum mismatch - refusing to install.`n  expected $expected`n  actual   $actual"
    }
    Info "sha256 ok"
}

# --- 4. Install -------------------------------------------------------------
Step "Installing to $InstallDir"
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
# Overwriting a RUNNING gritty.exe fails with a sharing violation, but
# Windows allows renaming one - move it aside so upgrading while gritty is
# open just works; the stale .old is swept on the next install.
try {
    # Sweep stale asides from previous upgrades (in-use ones survive until a
    # later run when their process has exited).
    Get-ChildItem "$ExePath.old*" -ErrorAction SilentlyContinue |
        Remove-Item -Force -ErrorAction SilentlyContinue
    if (Test-Path $ExePath) {
        # Unique name: an older aside may still be mapped by a live process.
        Move-Item -Force $ExePath ("$ExePath.old." + [Guid]::NewGuid().ToString('N').Substring(0, 8))
    }
    Move-Item -Force $tmp $ExePath
} catch {
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    Die "Could not write $ExePath. Close any running gritty and re-run. ($_)"
}
Done "installed gritty $tag"

# --- 5. Shortcuts -----------------------------------------------------------
if ($env:GRITTY_NO_SHORTCUT -ne '1') {
    Step "Creating shortcuts"
    $shell = New-Object -ComObject WScript.Shell
    $startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\gritty.lnk'
    $desktop   = Join-Path ([Environment]::GetFolderPath('Desktop')) 'gritty.lnk'
    foreach ($lnkPath in @($startMenu, $desktop)) {
        $lnk = $shell.CreateShortcut($lnkPath)
        $lnk.TargetPath       = $ExePath
        $lnk.WorkingDirectory = $InstallDir
        $lnk.IconLocation     = "$ExePath,0"
        $lnk.Description       = 'gritty - native Windows terminal multiplexer'
        $lnk.Save()
    }
    Info "Start Menu + Desktop"
}

# --- 6. PATH ----------------------------------------------------------------
if ($env:GRITTY_NO_PATH -ne '1') {
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    if (($userPath -split ';') -notcontains $InstallDir) {
        Step "Adding to PATH"
        $newPath = if ([string]::IsNullOrEmpty($userPath)) { $InstallDir } else { "$userPath;$InstallDir" }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        Info "open a new terminal to pick up 'gritty' on PATH"
    }
}

Write-Host ""
Done "gritty is ready"
Write-Host ""
Write-Host "  Launch it from the Start Menu, the Desktop shortcut, or by running" -ForegroundColor Gray
Write-Host "  'gritty' in a new terminal. Closing that terminal won't close gritty." -ForegroundColor Gray
Write-Host ""
