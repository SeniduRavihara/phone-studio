#!/usr/bin/env bash
set -euo pipefail
app_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "$app_dir/dist/phone-studio" ]]; then
    exec "$app_dir/dist/phone-studio" "$@"
fi
cargo_bin="${CARGO:-cargo}"
if ! command -v "$cargo_bin" >/dev/null 2>&1 && [[ -x "$HOME/.cargo/bin/cargo" ]]; then
    cargo_bin="$HOME/.cargo/bin/cargo"
fi
exec "$cargo_bin" run --locked --manifest-path "$app_dir/Cargo.toml" -- "$@"
