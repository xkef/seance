#!/usr/bin/env bash
# Run every CI gate locally: fmt, clippy, tests, markdown.
set -euo pipefail
cd "$(dirname "$0")/.."
command -v cargo-nextest >/dev/null 2>&1 || cargo install --locked cargo-nextest
bash tools/check-naming.sh
cargo fmt-check
cargo lint
cargo nextest run --workspace
bash tools/md-check.sh
