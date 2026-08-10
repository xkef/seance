#!/usr/bin/env bash
# Prepare a local ghostty source tree for libghostty-vt-sys to build against.
#
# Rationale:
#   libghostty-vt-sys/build.rs fetches ghostty at a pinned commit on every
#   fresh OUT_DIR (i.e. every `cargo clean`, every new profile, every CI run).
#   Two issues follow:
#     1. The pinned ghostty commit's build.zig unconditionally runs
#        `xcodebuild -create-xcframework` on Darwin. That step fails on hosts
#        with a broken Xcode plugin load (IDESimulatorFoundation) even though
#        the dylib we actually consume was already produced.
#     2. Re-fetching the same commit into each OUT_DIR wastes time and disk.
#   Solution: maintain one repo-local clone at vendor/ghostty-src, pre-patched
#   to skip the xcframework step. `.cargo/config.toml` sets
#   GHOSTTY_SOURCE_DIR so build.rs uses this clone instead of fetching.
#
# Usage: tools/setup-ghostty-src.sh
#   - Clones on first run, re-clones on commit mismatch, idempotent otherwise.
#   - Run it after `cargo clean`, after bumping the libghostty-vt dep, or
#     after changing this script's GHOSTTY_COMMIT.

set -euo pipefail

# MUST match libghostty-vt-sys/build.rs's GHOSTTY_COMMIT (for the pinned
# libghostty-vt git revision in Cargo.toml). Mismatch means the FFI bindings
# in libghostty-vt-sys/src/bindings.rs will disagree with the C headers
# produced by zig build, resulting in link errors or UB at runtime.
GHOSTTY_COMMIT="6590196661f769dd8f2b3e85d6c98262c4ec5b3b"
GHOSTTY_REPO="https://github.com/ghostty-org/ghostty.git"

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO/vendor/ghostty-src"
STAMP="$SRC/.seance-commit"
PATCH_STAMP="$SRC/.seance-xcframework-patched"

mkdir -p "$REPO/vendor"

need_reclone=0
if [ ! -d "$SRC/.git" ]; then
    need_reclone=1
elif [ ! -f "$STAMP" ] || [ "$(cat "$STAMP" 2>/dev/null)" != "$GHOSTTY_COMMIT" ]; then
    need_reclone=1
fi

if [ "$need_reclone" = 1 ]; then
    rm -rf "$SRC"
    echo "Cloning ghostty $GHOSTTY_COMMIT into vendor/ghostty-src ..."
    git clone --quiet --filter=blob:none --no-checkout "$GHOSTTY_REPO" "$SRC"
    git -C "$SRC" -c advice.detachedHead=false checkout --quiet "$GHOSTTY_COMMIT"
    echo "$GHOSTTY_COMMIT" > "$STAMP"
    rm -f "$PATCH_STAMP"
fi

if [ ! -f "$PATCH_STAMP" ]; then
    # Rewrite the xcframework `if` guard so it becomes a dead branch.
    # Idempotent by design: `grep -q` detects an already-patched file.
    if ! grep -q 'false and builtin.os.tag.isDarwin' "$SRC/build.zig"; then
        # Route through a temp file: BSD sed (`-i ''`) and GNU sed (`-i`) take
        # incompatible in-place flags, so `sed -i ''` aborts on Linux and, under
        # `set -e`, kills setup before the build ever runs.
        tmp="$(mktemp)"
        sed \
            's|if (builtin.os.tag.isDarwin() and config.target.result.os.tag.isDarwin())|if (false and builtin.os.tag.isDarwin() and config.target.result.os.tag.isDarwin())|' \
            "$SRC/build.zig" > "$tmp"
        mv "$tmp" "$SRC/build.zig"
        echo "Patched vendor/ghostty-src/build.zig: xcframework step disabled."
    fi
    touch "$PATCH_STAMP"
fi

echo "vendor/ghostty-src ready at $GHOSTTY_COMMIT"
