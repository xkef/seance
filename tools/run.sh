#!/usr/bin/env bash
# Build + bundle + launch Seance.app via launchd.
# Use this instead of `cargo run` on macOS — terminal-parented processes
# can't reliably take focus on some machines (macOS 26 responsibility inheritance).
set -euo pipefail
cd "$(dirname "$0")/.."
bash tools/setup-ghostty-src.sh >/dev/null
bash tools/setup-themes.sh >/dev/null
bash tools/setup-sysroot.sh >/dev/null
PROFILE="${1:-release}"
cargo build ${PROFILE:+--profile=$PROFILE} 2>/dev/null || cargo build --release
bash tools/make-app.sh "$PROFILE" >/dev/null
open "target/Seance.app"
