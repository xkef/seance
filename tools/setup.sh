#!/usr/bin/env bash
# Vendored toolchain setup: ghostty-src + themes + macOS sysroot.
# Re-run after `cargo clean` or after bumping libghostty-vt in Cargo.toml.
set -euo pipefail
cd "$(dirname "$0")/.."
bash tools/setup-ghostty-src.sh
bash tools/setup-themes.sh
bash tools/setup-sysroot.sh
