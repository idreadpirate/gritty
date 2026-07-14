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
function Warn($m) { Write-Host "[warn] $m" -ForegroundColor Yellow }
function Die($m)  { Write-Host "[error] $m" -ForegroundColor Red; exit 1 }

# SHA256 of a file. Get-FileHash can be unavailable when PSModulePath is
# overridden by a hosting environment (nested shells, CI, IDE tasks) — fall
# back to certutil, which ships with every Windows.
function Get-Sha256([string]$Path) {
    if (Get-Command Get-FileHash -ErrorAction SilentlyContinue) {
        return (Get-FileHash -Algorithm SHA256 $Path).Hash.ToLower()
    }
    $line = & certutil -hashfile $Path SHA256 2>$null |
        Where-Object { ($_ -replace '\s', '') -match '^[0-9a-fA-F]{64}$' } |
        Select-Object -First 1
    if (-not $line) { Die "Could not compute SHA256 (no Get-FileHash; certutil failed)." }
    return ($line -replace '\s', '').ToLower()
}

Write-Host ""
Write-Host "  gritty installer" -ForegroundColor White
Write-Host ""

# --- 0. CPU preflight ---------------------------------------------------------
# The released exe targets x86-64-v3 (AVX2, Haswell 2013+). A CPU without AVX2
# would die on launch with an illegal-instruction crash and no message — refuse
# up front where the runtime can tell us (pwsh 7+). Windows PowerShell 5.1 has
# no reliable check, so it proceeds (AVX2 is near-universal on Win10/11-era
# hardware). Building from source with a lower target-cpu always works.
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -eq 'ARM64') {
    Warn "ARM64 Windows detected: gritty.exe is x64 + AVX2; x64 emulation without AVX2 (before Windows 11 24H2) will crash on launch."
} elseif ($PSVersionTable.PSEdition -eq 'Core') {
    try {
        if (-not [System.Runtime.Intrinsics.X86.Avx2]::IsSupported) {
            Die "This CPU reports no AVX2 support; the released gritty.exe requires it (x86-64-v3, ~2013+). Build from source with a lower target-cpu instead."
        }
    } catch { <# intrinsics API unavailable — proceed like 5.1 #> }
}

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
    $actual   = Get-Sha256 $tmp
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
