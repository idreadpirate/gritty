# gate.ps1 — the no-bloat / quality gate. Every hardening iteration must pass
# this or its changes are reverted. Exits nonzero on any violation.

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$MaxBytes = 810000    # binary ceiling — gritty is a *minimal* terminal. Held under
                      # 810 KB even after the 2026-06 hardening features (HiDPI, IME,
                      # config, dirty-rect, crash-log, CJK) and the agent-awareness
                      # feature (per-pane agent detection + overview overlay + hang
                      # watchdog) via: opt-level=z, hand-rolled config + session
                      # parsers (no toml / serde_json in the runtime), a 32px icon,
                      # and nightly -Z build-std (std rebuilt at opt=z). See
                      # .cargo/config.toml + rust-toolchain.toml.
$MaxPkgs  = 290       # Cargo.lock package ceiling — dependency guard

function Fail($m) { Write-Host "GATE FAIL: $m" -ForegroundColor Red; exit 1 }

Write-Host "[1/6] fmt";    cargo fmt --check;                     if ($LASTEXITCODE) { Fail "rustfmt" }
Write-Host "[2/6] clippy"; cargo clippy --all-targets -- -D warnings; if ($LASTEXITCODE) { Fail "clippy" }
Write-Host "[3/6] test";   cargo test --quiet;                   if ($LASTEXITCODE) { Fail "tests" }
Write-Host "[4/6] build";  cargo build --release --quiet;        if ($LASTEXITCODE) { Fail "build" }

Write-Host "[5/6] size"
$sz = (Get-Item target/x86_64-pc-windows-msvc/release/gritty.exe).Length
if ($sz -gt $MaxBytes) { Fail "binary $sz > $MaxBytes bytes (bloat)" }

Write-Host "[6/6] deps"
$pkgs = (Select-String -Path Cargo.lock -Pattern '^name = ').Count
if ($pkgs -gt $MaxPkgs) { Fail "deps $pkgs > $MaxPkgs (bloat)" }

Write-Host "GATE PASS  binary=$sz bytes  deps=$pkgs" -ForegroundColor Green
