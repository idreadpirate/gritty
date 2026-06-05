# gate.ps1 — the no-bloat / quality gate. Every hardening iteration must pass
# this or its changes are reverted. Exits nonzero on any violation.

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

$MaxBytes = 1500000   # binary ceiling. The size budget was deliberately traded for
                      # SPEED in the throughput/memory pass: opt-level=3 +
                      # target-cpu=x86-64-v3 (AVX2) + std rebuilt for speed (see
                      # .cargo/config.toml + docs/ARCHITECTURE.md). The binary grew
                      # from ~800 KB (opt=z) to ~1.1-1.4 MB; this ceiling guards
                      # against unbounded further bloat, not for a minimal binary.
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
