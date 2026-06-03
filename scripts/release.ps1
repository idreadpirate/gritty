# release.ps1 — build, gate, and publish a gritty release to GitHub.
#
#   ./scripts/release.ps1            # tag from Cargo.toml version (v<version>)
#   ./scripts/release.ps1 -Tag v0.2.0
#
# Produces the assets that install.ps1 downloads: gritty.exe and
# gritty.exe.sha256. Requires the GitHub CLI (`gh auth login` once).

param(
    [string]$Tag,
    [switch]$Draft
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path $PSScriptRoot -Parent)

function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Die($m)  { Write-Host "[error] $m" -ForegroundColor Red; exit 1 }

# --- Preconditions ----------------------------------------------------------
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Die "GitHub CLI 'gh' not found. Install it and run 'gh auth login'."
}

# --- Resolve the version/tag ------------------------------------------------
if (-not $Tag) {
    $verLine = Select-String -Path Cargo.toml -Pattern '^\s*version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if (-not $verLine) { Die "Could not read version from Cargo.toml." }
    $version = $verLine.Matches[0].Groups[1].Value
    $Tag = "v$version"
}
Step "Releasing $Tag"

# --- Quality gate (fmt + clippy + tests + budgets) + release build ----------
Step "Running quality gate"
& "$PSScriptRoot\gate.ps1"
if ($LASTEXITCODE) { Die "Quality gate failed — release aborted." }

# build-std pins the MSVC target, so the release exe lands under the target triple.
$exe = "target/x86_64-pc-windows-msvc/release/gritty.exe"
if (-not (Test-Path $exe)) { Die "$exe not found after build." }

# --- Stage assets (exe + checksum) ------------------------------------------
Step "Computing checksum"
$staging = Join-Path ([IO.Path]::GetTempPath()) "gritty-release-$Tag"
New-Item -ItemType Directory -Force -Path $staging | Out-Null
$stagedExe = Join-Path $staging 'gritty.exe'
Copy-Item -Force $exe $stagedExe
$hash = (Get-FileHash -Algorithm SHA256 $stagedExe).Hash.ToLower()
"$hash  gritty.exe" | Set-Content -NoNewline (Join-Path $staging 'gritty.exe.sha256')
Write-Host "  sha256 $hash" -ForegroundColor Gray

# --- Publish ----------------------------------------------------------------
Step "Creating GitHub release"
$notes = "gritty $Tag`n`nInstall:`n``````powershell`nirm https://raw.githubusercontent.com/idreadpirate/gritty/master/scripts/install.ps1 | iex`n``````"
$args = @(
    'release', 'create', $Tag,
    (Join-Path $staging 'gritty.exe'),
    (Join-Path $staging 'gritty.exe.sha256'),
    '--title', "gritty $Tag",
    '--notes', $notes
)
if ($Draft) { $args += '--draft' }
& gh @args
if ($LASTEXITCODE) { Die "gh release create failed." }

Remove-Item $staging -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "[ok] published $Tag" -ForegroundColor Green
