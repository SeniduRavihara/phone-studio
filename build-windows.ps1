$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') {
    throw 'Run this build script on Windows with Rust MSVC and C++ Build Tools installed.'
}
$target = Join-Path $PSScriptRoot 'target'
& cargo build --release --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml') --target-dir $target
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
$dist = Join-Path $PSScriptRoot 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null
Copy-Item (Join-Path $target 'release/phone-studio.exe') (Join-Path $dist 'phone-studio.exe') -Force
Copy-Item (Join-Path $PSScriptRoot 'README.md') (Join-Path $dist 'README.md') -Force
Write-Host "Built $dist/phone-studio.exe. Install official Platform-Tools and the appropriate phone driver separately."
