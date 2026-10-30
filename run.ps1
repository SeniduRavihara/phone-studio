$ErrorActionPreference = 'Stop'
$binary = Join-Path $PSScriptRoot 'dist/phone-studio.exe'
if (Test-Path $binary) {
    & $binary @args
} else {
    & cargo run --locked --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml') -- @args
}
exit $LASTEXITCODE
