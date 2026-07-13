# stress.ps1 — gritty load / leak harness.
#
# What it does:
#   1. Generates a session.json with N panes (split across tabs) and a config
#      that pins the shell to Windows PowerShell, so every pane is a known shell.
#   2. Launches the release gritty.exe; it restores all N panes at once
#      (one ConPTY + reader thread + scrollback grid per pane).
#   3. Optionally (-Broadcast) drives a streaming spinner into EVERY pane at once
#      via gritty's Ctrl+Shift+B broadcast-paste — the render/dirty-rect stress.
#   4. Samples gritty's RSS / private bytes / thread + handle count / CPU over
#      time, writes a CSV, and prints a PASS/FAIL verdict:
#        * leak  -> resident memory keeps climbing after warm-up
#        * threads/handles -> climb while pane count is fixed (leaked PTY readers)
#        * spin  -> CPU stays pegged under streaming (dirty-rect regression)
#   5. Restores your real session.json / config.toml.
#
# Existing session.json and config.toml are backed up to *.harnessbak and
# restored in a finally{} block, even on Ctrl+C.
#
# Usage:
#   pwsh -File scripts/stress.ps1                       # 100 panes, idle, 60s
#   pwsh -File scripts/stress.ps1 -Broadcast            # + stream into all panes
#   pwsh -File scripts/stress.ps1 -Panes 64 -PerTab 16 -Seconds 90 -Broadcast
#   pwsh -File scripts/stress.ps1 -Throughput -FloodMB 50 -Exe <path>   # speed A/B
#   pwsh -File scripts/stress.ps1 -DryRun               # generate+validate only
#
# -Throughput: one pane, flood a fixed payload, report MB/s rendered + CPU-ms per
# MB + peak RSS. Run it twice with two different -Exe builds (e.g. an opt=z
# baseline vs the opt=3 build) to measure a speedup: higher MB/s, lower CPU-ms/MB.
#
# The GUI + N shells run on YOUR desktop; this is a desktop tool, not a headless
# one. -DryRun generates and validates the files without launching anything.

[CmdletBinding()]
param(
    [int]$Panes      = 100,
    [int]$PerTab     = 20,
    [int]$Seconds    = 60,
    [int]$IntervalMs = 1500,
    [string]$Exe     = "target/x86_64-pc-windows-msvc/release/gritty.exe",
    [switch]$Broadcast,
    [switch]$MultiTab,
    [int]$Solo = -1,
    [int]$LoadAll = -1,
    [switch]$Throughput,
    [int]$FloodMB    = 50,
    [switch]$DryRun,
    [string]$LogDir  = "$env:TEMP\gritty-stress"
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)   # repo root

function Info($m) { Write-Host $m -ForegroundColor Cyan }
function Warn($m) { Write-Host "WARN: $m" -ForegroundColor Yellow }
function Fail($m) { Write-Host "STRESS FAIL: $m" -ForegroundColor Red; exit 1 }

if ($Panes -lt 1)  { Fail "-Panes must be >= 1" }
if ($PerTab -lt 1) { $PerTab = $Panes }

# -Throughput is a focused speed A/B: one pane, flood a fixed payload, measure how
# fast (and how cheaply) gritty renders it. Pass two different -Exe builds to
# compare. It overrides the multi-pane leak layout.
if ($Throughput) { $Panes = 1; $PerTab = 1 }
# -Solo N: leak isolation — a single pane running ONLY MultiTab workload N
# (0=spinner, 1=scrollflood, 2=colorflood, 3=titlestorm). One variable at a time.
if ($Solo -ge 0) { $Panes = 1; $PerTab = 1 }

# ---------------------------------------------------------------------------
# JSON generation: a balanced split tree of `n` leaf panes (ids 0..n-1),
# alternating split axis by depth so panes tile into a grid.
# ---------------------------------------------------------------------------
function New-PaneTree {
    param([int[]]$Ids, [int]$Depth = 0)
    if ($Ids.Count -eq 1) { return "{""Leaf"":$($Ids[0])}" }
    $mid   = [int]([math]::Floor($Ids.Count / 2))
    $left  = $Ids[0..($mid - 1)]
    $right = $Ids[$mid..($Ids.Count - 1)]
    $axis  = if ($Depth % 2 -eq 0) { "LeftRight" } else { "TopBottom" }
    $a = New-PaneTree -Ids $left  -Depth ($Depth + 1)
    $b = New-PaneTree -Ids $right -Depth ($Depth + 1)
    return "{""Split"":{""axis"":""$axis"",""ratio"":0.5,""a"":$a,""b"":$b}}"
}

# Industrial palette mirroring TAB_PALETTE in main.rs.
$palette = @(0xff7b00, 0xe69522, 0xc08a4a, 0xb87333, 0x7fa6c9, 0xd4a017)

# Build tabs: fill PerTab panes per tab until `Panes` total are placed.
$tabsJson = New-Object System.Collections.Generic.List[string]
$placed = 0
$tabIndex = 0
while ($placed -lt $Panes) {
    $count = [math]::Min($PerTab, $Panes - $placed)
    $ids = 0..($count - 1)
    $tree = New-PaneTree -Ids $ids
    $panesJson = ($ids | ForEach-Object { "{""id"":$_,""name"":""p$_""}" }) -join ","
    $color = $palette[$tabIndex % $palette.Count]
    $tab = "{""name"":""t$tabIndex"",""color"":$color,""focus"":0,""next_id"":$count,""tree"":$tree,""panes"":[$panesJson]}"
    $tabsJson.Add($tab)
    $placed += $count
    $tabIndex++
}

$winW = 1600
$winH = 1000
$windowJson = "{""active"":0,""tabs"":[$([string]::Join(",", $tabsJson))],""win_w"":$winW,""win_h"":$winH,""win_x"":null,""win_y"":null,""seamless"":false}"
$sessionJson = "{""windows"":[$windowJson],""active"":0,""tabs"":[],""win_w"":null,""win_h"":null}"

# Validate the JSON we just hand-built actually parses.
try { $null = $sessionJson | ConvertFrom-Json } catch { Fail "generated session.json is not valid JSON: $_" }
Info "Generated session: $Panes panes across $tabIndex tab(s), $PerTab per tab."

# Pin the shell to Windows PowerShell so broadcast commands have known syntax.
$pwshExe = (Get-Command powershell.exe -ErrorAction SilentlyContinue).Source
if (-not $pwshExe) { Fail "powershell.exe not found on PATH" }
$configText = "shell = ""$($pwshExe -replace '\\','\\')""`nscrollback = 5000`n"

# ---------------------------------------------------------------------------
# File paths + backup/restore (matches src/persist.rs + src/config.rs).
# ---------------------------------------------------------------------------
$sessionPath = Join-Path $env:LOCALAPPDATA "gritty\session.json"
$configPath  = Join-Path $env:APPDATA      "gritty\config.toml"
New-Item -ItemType Directory -Force -Path (Split-Path $sessionPath) | Out-Null
New-Item -ItemType Directory -Force -Path (Split-Path $configPath)  | Out-Null
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

# Record the exact pre-run state: the file itself, or an explicit "was absent"
# marker. Restore replays that record verbatim. The marker gates the only
# deletion path, so a Restore can never destroy a file whose pre-run existence
# we did not personally witness (e.g. after a manual mid-run intervention).
function Backup($p) {
    if (Test-Path $p) { Copy-Item $p "$p.harnessbak" -Force }
    else { New-Item -ItemType File -Path "$p.harnessnone" -Force | Out-Null }
}
function Restore($p) {
    if (Test-Path "$p.harnessbak") { Move-Item "$p.harnessbak" $p -Force }
    elseif (Test-Path "$p.harnessnone") {
        Remove-Item "$p.harnessnone" -Force
        if (Test-Path $p) { Remove-Item $p -Force }
    }
    # No record at all: leave the live file untouched.
}
# BOM-less UTF-8 writes: PowerShell 5.1's `Set-Content -Encoding UTF8` prepends
# a BOM. gritty now tolerates that, but the harness should still write clean
# files under every PowerShell host.
function WriteNoBom($p, $text) {
    [System.IO.File]::WriteAllText($p, $text, (New-Object System.Text.UTF8Encoding($false)))
}

if ($DryRun) {
    $sDry = Join-Path $LogDir "session.dryrun.json"
    $cDry = Join-Path $LogDir "config.dryrun.toml"
    Set-Content -Path $sDry -Value $sessionJson -Encoding UTF8
    Set-Content -Path $cDry -Value $configText  -Encoding UTF8
    Info "DryRun: wrote $sDry ($([math]::Round((Get-Item $sDry).Length/1kb,1)) KB) and $cDry"
    Info "DryRun: session.json parsed OK; shell -> $pwshExe"
    Info "DryRun: no process launched, no files in the live gritty dirs touched."
    exit 0
}

$exePath = if ([System.IO.Path]::IsPathRooted($Exe)) { $Exe } else { Join-Path (Get-Location) $Exe }
if (-not (Test-Path $exePath)) { Fail "gritty exe not found: $exePath  (build it: cargo build --release)" }

# ---------------------------------------------------------------------------
# Process discovery. Release gritty self-detaches (main.rs ensure_detached):
# the launched process exits and a new detached gritty.exe is the real one, so
# we find it by name, not by the Start-Process handle.
# ---------------------------------------------------------------------------
function Get-GrittyProcs { Get-Process -Name gritty -ErrorAction SilentlyContinue }

# GDI/USER object counts per process (GetGuiResources). A GDI leak is the classic
# "lags badly after an hour" failure on Windows — rendering slows to a crawl as
# the process nears the 10k GDI-object limit — and it is invisible in RSS,
# HandleCount (kernel handles only), and thread count, so sample it explicitly.
Add-Type @"
using System;using System.Runtime.InteropServices;
public class Gui { [DllImport("user32.dll")] public static extern uint GetGuiResources(IntPtr h, uint flags); }
"@ -ErrorAction SilentlyContinue

function Sample {
    $ps = Get-GrittyProcs
    if (-not $ps) { return $null }
    $rss   = ($ps | Measure-Object WorkingSet64        -Sum).Sum
    $priv  = ($ps | Measure-Object PrivateMemorySize64 -Sum).Sum
    $thr   = ($ps | Measure-Object -Property Threads -Sum -ErrorAction SilentlyContinue).Sum
    if (-not $thr) { $thr = ($ps | ForEach-Object { $_.Threads.Count } | Measure-Object -Sum).Sum }
    $hnd   = ($ps | Measure-Object HandleCount -Sum).Sum
    $cpu   = ($ps | Measure-Object CPU -Sum).Sum
    $gdi = 0; $usr = 0
    foreach ($p in $ps) {
        try {
            $gdi += [Gui]::GetGuiResources($p.Handle, 0)   # GR_GDIOBJECTS
            $usr += [Gui]::GetGuiResources($p.Handle, 1)   # GR_USEROBJECTS
        } catch {}
    }
    [pscustomobject]@{
        RssMB   = [math]::Round($rss  / 1mb, 1)
        PrivMB  = [math]::Round($priv / 1mb, 1)
        Threads = [int]$thr
        Handles = [int]$hnd
        Gdi     = [int]$gdi
        User    = [int]$usr
        CpuSec  = [math]::Round($cpu, 2)
        Count   = $ps.Count
    }
}

Backup $sessionPath
Backup $configPath

# MultiTab/Solo workload injection — deterministic, focus-independent.
# The pinned Windows PowerShell loads its per-user profile (the harness spawn
# uses -NoLogo, not -NoProfile); a temporary profile runs the workload script
# named by GRITTY_STRESS_CMD, which panes inherit from the harness-launched
# gritty. Restored (or removed) in the finally block like session/config.
$profPath = $null
if ($Throughput) {
    # Fixed-payload flood, injected at shell spawn via the profile (below) —
    # no clipboard/SendKeys/foreground dependency, so A/B runs are identical
    # and unattended-safe. Timing starts at process detection.
    $lineBytes = 1024
    $lines = [int]([math]::Floor($FloodMB * 1MB / $lineBytes))
    $fillN = $lineBytes - 15
    $floodCmd = '$e=[char]27;$l=("{0}[38;5;208m{1}{0}[0m" -f $e,(''x''*' + $fillN + '));for($i=0;$i-lt ' + $lines + ';$i++){[Console]::Out.WriteLine($l)};[Console]::Out.WriteLine(''__FLOOD_DONE__'')'
    WriteNoBom (Join-Path $LogDir "wflood.ps1") $floodCmd
    $profDir  = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'WindowsPowerShell'
    $profPath = Join-Path $profDir 'Microsoft.PowerShell_profile.ps1'
    New-Item -ItemType Directory -Force -Path $profDir | Out-Null
    Backup $profPath
    WriteNoBom $profPath 'if ($env:GRITTY_STRESS_CMD) { Invoke-Expression $env:GRITTY_STRESS_CMD }'
    $env:GRITTY_STRESS_CMD = ". '$(Join-Path $LogDir "wflood.ps1")'"
}
elseif ($MultiTab -or $Solo -ge 0) {
    $runFor = $Seconds + 20
    $workloads = @(
        # 0: CR spinner - dirty-rect single-row repaint path.
        ('$e=(Get-Date).AddSeconds(' + $runFor + ');$s=''|'',''/'',''-'','''';$i=0;while((Get-Date)-lt $e){$i++;Write-Host -NoNewline ("`r[{0}] fg-spin {1} " -f $s[$i%4],$i);Start-Sleep -Milliseconds 40}'),
        # 1: full-rate scroll flood - history/ring churn + max wake rate.
        ('$e=(Get-Date).AddSeconds(' + $runFor + ');$i=0;while((Get-Date)-lt $e){$i++;[Console]::Out.WriteLine(("scrollflood {0} " -f $i).PadRight(120,''x''))}'),
        # 2: SGR color flood - styled cells + scroll.
        ('$e=(Get-Date).AddSeconds(' + $runFor + ');$c=[char]27;$i=0;while((Get-Date)-lt $e){$i++;[Console]::Out.WriteLine(("{0}[38;5;{1}mcolor {2}{0}[0m" -f $c,($i%230+1),$i))}'),
        # 3: title + OSC churn - OSC 0/2 storm.
        ('$e=(Get-Date).AddSeconds(' + $runFor + ');$c=[char]27;$i=0;while((Get-Date)-lt $e){$i++;[Console]::Out.Write(("{0}]0;title {1}{2}" -f $c,$i,[char]7));if($i%50-eq0){[Console]::Out.WriteLine("t $i")};Start-Sleep -Milliseconds 5}')
    )
    for ($i = 0; $i -lt $workloads.Count; $i++) {
        WriteNoBom (Join-Path $LogDir "w$i.ps1") $workloads[$i]
    }
    $profDir  = Join-Path ([Environment]::GetFolderPath('MyDocuments')) 'WindowsPowerShell'
    $profPath = Join-Path $profDir 'Microsoft.PowerShell_profile.ps1'
    New-Item -ItemType Directory -Force -Path $profDir | Out-Null
    Backup $profPath
    WriteNoBom $profPath 'if ($env:GRITTY_STRESS_CMD) { Invoke-Expression $env:GRITTY_STRESS_CMD }'
    $sel = if ($Solo -ge 0) { [math]::Min($Solo, 3) }
           elseif ($LoadAll -ge 0) { [math]::Min($LoadAll, 3) }
           else { $null }
    $env:GRITTY_STRESS_CMD = if ($null -ne $sel) { ". '$(Join-Path $LogDir "w$sel.ps1")'" }
                             else { ". (Join-Path '$LogDir' ('w{0}.ps1' -f (Get-Random -Maximum 4)))" }
}
$proc = $null
$samples = New-Object System.Collections.Generic.List[object]
try {
    WriteNoBom $sessionPath $sessionJson
    WriteNoBom $configPath  $configText

    Info "Launching $exePath  ($Panes panes)..."
    $script:launchedAt = Get-Date
    Start-Process -FilePath $exePath | Out-Null

    # Wait for the detached gritty to appear.
    $deadline = (Get-Date).AddSeconds(15)
    while (-not (Get-GrittyProcs) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 250 }
    $g = Get-GrittyProcs
    if (-not $g) { Fail "gritty did not start within 15s" }
    Info "gritty up (pid(s): $(($g | ForEach-Object Id) -join ', ')). Letting $Panes shells settle..."
    if (-not $Throughput) { Start-Sleep -Seconds 5 }   # settle; Throughput floods from spawn

    if ($Throughput) {
        # Speed A/B: the profile-injected flood started when the pane's shell
        # spawned. Measure from launch to the CPU plateau (drained); shell boot
        # (~1 s) is included identically in every run, so A/B stays fair.
        $cpu0 = 0.0
        $t0 = $script:launchedAt
        Info ("Flooding ~{0} MB ({1} lines) into one pane; measuring drain..." -f $FloodMB, $lines)

        $peakRssMB = 0.0; $lastCpu = $cpu0; $idleHits = 0; $busySeen = $false
        $plateauAt = $null
        $tpDeadline = $t0.AddSeconds(120)
        while ((Get-Date) -lt $tpDeadline) {
            Start-Sleep -Milliseconds 300
            $s = Sample
            if (-not $s) { break }
            if ($s.RssMB -gt $peakRssMB) { $peakRssMB = $s.RssMB }
            $d = $s.CpuSec - $lastCpu
            $lastCpu = $s.CpuSec
            if ($d -gt 0.15) { $busySeen = $true; $idleHits = 0 }
            elseif ($busySeen) { $idleHits++; if ($idleHits -ge 3) { $plateauAt = Get-Date; break } }
        }
        if (-not $plateauAt) { $plateauAt = Get-Date }
        $cpuRender = [math]::Round($lastCpu - $cpu0, 2)
        $wall = [math]::Round(($plateauAt - $t0).TotalSeconds, 2)
        $actualMB = [math]::Round(($lines * ($lineBytes + 2)) / 1MB, 1)   # +2 CRLF
        $script:tp = [pscustomobject]@{
            payloadMB  = $actualMB
            wallS      = $wall
            cpuRender  = $cpuRender
            MBps       = if ($wall -gt 0)     { [math]::Round($actualMB / $wall, 1) }        else { 0 }
            cpuMsPerMB = if ($actualMB -gt 0) { [math]::Round($cpuRender * 1000 / $actualMB, 1) } else { 0 }
            peakRssMB  = [math]::Round($peakRssMB, 1)
            drained    = ($idleHits -ge 3)
        }
    }
    elseif ($MultiTab -or $Solo -ge 0) {
        # Workloads were injected via the temporary PowerShell profile before
        # launch (see the pre-launch block) — nothing to drive here.
        if ($Solo -ge 0) { Info "Solo: single pane running workload #$Solo via profile injection." }
        elseif ($LoadAll -ge 0) { Info "LoadAll: every pane running workload #$LoadAll via profile injection." }
        else { Info "MultiTab: every pane running a random workload (0-3) via profile injection." }
    }
    elseif ($Broadcast) {
        # Drive a self-terminating streaming spinner into EVERY pane at once.
        # CR-rewrites one line (the dirty-rect partial-repaint happy path) plus a
        # periodic newline (scroll). Bounded so orphaned panes self-exit.
        $runFor = $Seconds + 20
        $cmd = '$e=(Get-Date).AddSeconds(' + $runFor + ');$s=''|'',''/'',''-'',''\'';$i=0;while((Get-Date)-lt $e){$i++;Write-Host -NoNewline ("`r[{0}] gritty-stress {1} " -f $s[$i%4],$i);if($i%40-eq0){Write-Host ("`n line {0}" -f $i)};Start-Sleep -Milliseconds 40}'
        Set-Clipboard -Value ($cmd + "`r`n")
        Add-Type -AssemblyName System.Windows.Forms
        Add-Type @"
using System;using System.Runtime.InteropServices;
public class Fg { [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
[DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h,int n); }
"@
        $hwnd = ($g | Where-Object MainWindowHandle -ne 0 | Select-Object -First 1).MainWindowHandle
        if ($hwnd) {
            [Fg]::ShowWindow($hwnd, 9) | Out-Null   # SW_RESTORE
            [Fg]::SetForegroundWindow($hwnd) | Out-Null
            Start-Sleep -Milliseconds 600
            [System.Windows.Forms.SendKeys]::SendWait("^+b")  # Ctrl+Shift+B broadcast-paste
            Info "Broadcast-pasted a streaming spinner into all panes (Ctrl+Shift+B)."
        } else {
            Warn "no window handle yet; skipping broadcast (memory test still runs)."
        }
    }

    # ---- sample loop ---- (skipped under -Throughput, which measured its own way)
    if (-not $Throughput) {
    Info ("Sampling every {0} ms for {1}s..." -f $IntervalMs, $Seconds)
    $startTime = Get-Date
    $end = $startTime.AddSeconds($Seconds)
    while ((Get-Date) -lt $end) {
        $s = Sample
        if ($s) {
            $samples.Add($s)
            $elapsed = [int]((Get-Date) - $startTime).TotalSeconds
            Write-Host ("  t+{0,3}s  rss={1,7} MB  priv={2,7} MB  thr={3,4}  hnd={4,5}  gdi={5,5}  usr={6,4}  cpu={7,6}s  procs={8}" -f `
                $elapsed, $s.RssMB, $s.PrivMB, $s.Threads, $s.Handles, $s.Gdi, $s.User, $s.CpuSec, $s.Count)
        } else {
            Warn "gritty process vanished mid-run"
            break
        }
        Start-Sleep -Milliseconds $IntervalMs
    }
    }
}
finally {
    Info "Cleaning up..."
    Get-GrittyProcs | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 500
    Restore $sessionPath
    Restore $configPath
    if ($profPath) { Restore $profPath }
    $env:GRITTY_STRESS_CMD = $null
    Info "Restored your session.json and config.toml."
}

# ---------------------------------------------------------------------------
# Verdict.
# ---------------------------------------------------------------------------
if ($Throughput) {
    $t = $script:tp
    if (-not $t) { Fail "no throughput result captured (window never came up?)" }
    Write-Host ""
    Write-Host "==== gritty throughput ====" -ForegroundColor White
    Write-Host ("exe:        {0}" -f $exePath)
    Write-Host ("payload:    {0} MB  ({1})" -f $t.payloadMB, $(if ($t.drained) { "drained" } else { "TIMEOUT - raise -FloodMB budget or window" }))
    Write-Host ("wall:       {0} s" -f $t.wallS)
    Write-Host ("throughput: {0} MB/s rendered" -f $t.MBps) -ForegroundColor Green
    Write-Host ("CPU cost:   {0} CPU-s to render  =>  {1} CPU-ms per MB" -f $t.cpuRender, $t.cpuMsPerMB) -ForegroundColor Green
    Write-Host ("peak RSS:   {0} MB" -f $t.peakRssMB)
    Write-Host "A/B: run twice with different -Exe; higher MB/s and lower CPU-ms/MB = faster." -ForegroundColor Cyan
    exit 0
}

if ($samples.Count -lt 4) { Fail "too few samples ($($samples.Count)) to judge" }

$csv = Join-Path $LogDir ("run_{0:yyyyMMdd_HHmmss}.csv" -f (Get-Date))
$samples | Export-Csv -Path $csv -NoTypeInformation
Info "Samples written to $csv"

# Split warm-up (first third) from steady state (last two thirds).
$warm = [int]([math]::Floor($samples.Count / 3))
if ($warm -lt 1) { $warm = 1 }
$baseRss  = ($samples[0..($warm-1)] | Measure-Object RssMB   -Average).Average
$baseThr  = ($samples[0..($warm-1)] | Measure-Object Threads -Maximum).Maximum
$baseGdi  = ($samples[0..($warm-1)] | Measure-Object Gdi     -Maximum).Maximum
$endRss   = ($samples[($samples.Count-$warm)..($samples.Count-1)] | Measure-Object RssMB   -Average).Average
$endThr   = ($samples[($samples.Count-$warm)..($samples.Count-1)] | Measure-Object Threads -Maximum).Maximum
$endGdi   = ($samples[($samples.Count-$warm)..($samples.Count-1)] | Measure-Object Gdi     -Maximum).Maximum
$peakRss  = ($samples | Measure-Object RssMB -Maximum).Maximum

# CPU%: total CPU-seconds consumed across the run / wall-clock, x100.
$cpuDelta = $samples[-1].CpuSec - $samples[0].CpuSec
$wall     = ($samples.Count - 1) * ($IntervalMs / 1000.0)
$cpuPct   = if ($wall -gt 0) { [math]::Round(100.0 * $cpuDelta / $wall, 1) } else { 0 }

$rssGrowth = if ($baseRss -gt 0) { [math]::Round(100.0 * ($endRss - $baseRss) / $baseRss, 1) } else { 0 }

Write-Host ""
Write-Host "==== gritty stress summary ====" -ForegroundColor White
Write-Host ("panes={0}  samples={1}  wall={2}s" -f $Panes, $samples.Count, [int]$wall)
Write-Host ("RSS:     base={0} MB  end={1} MB  peak={2} MB  growth={3}%" -f $baseRss, $endRss, $peakRss, $rssGrowth)
Write-Host ("Threads: base(max)={0}  end(max)={1}" -f $baseThr, $endThr)
Write-Host ("GDI:     base(max)={0}  end(max)={1}" -f $baseGdi, $endGdi)
Write-Host ("CPU:     ~{0}% of one core over the run" -f $cpuPct)

# Thresholds (heuristic; eyeball the CSV too).
$leak   = $rssGrowth -gt 25            # resident set climbing after warm-up
$thrub  = ($endThr - $baseThr) -gt 5   # thread count growing while panes fixed
$gdileak = ($endGdi - $baseGdi) -gt 50 # GDI objects climbing while panes fixed
$verdict = $true
if ($leak)  { Write-Host "  -> possible LEAK: RSS grew $rssGrowth% after warm-up" -ForegroundColor Red; $verdict = $false }
if ($thrub) { Write-Host "  -> possible THREAD LEAK: +$($endThr-$baseThr) threads" -ForegroundColor Red; $verdict = $false }
if ($gdileak) { Write-Host "  -> possible GDI LEAK: +$($endGdi-$baseGdi) GDI objects (10k = UI death)" -ForegroundColor Red; $verdict = $false }
if ($Broadcast -and $cpuPct -gt 90) { Write-Host "  -> HIGH CPU under streaming ($cpuPct%): check dirty-rect" -ForegroundColor Yellow }

if ($verdict) {
    Write-Host "STRESS PASS  (RSS stable, no thread growth)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "STRESS FAIL" -ForegroundColor Red
    exit 1
}
