#!/bin/sh
# Creates tools/sysroot: a macOS SDK overlay with arm64-compatible libSystem.tbd.
# Run once after cloning, or after Xcode/SDK updates.
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SYSROOT="$SCRIPT_DIR/sysroot"
SDK_PATH="$(/usr/bin/xcrun --show-sdk-path)"
ZIG_LIBC="$(zig env 2>/dev/null | grep lib_dir | head -1 | sed 's/.*"\(.*\)".*/\1/')/libc/darwin/libSystem.tbd"

if [ ! -f "$ZIG_LIBC" ]; then
    echo "error: cannot find Zig's libSystem.tbd at $ZIG_LIBC" >&2
    exit 1
fi

rm -rf "$SYSROOT"
mkdir -p "$SYSROOT/usr/lib" "$SYSROOT/usr/include"

# Symlink SDK contents.
ln -sf "$SDK_PATH/usr/include"/* "$SYSROOT/usr/include/" 2>/dev/null || true
ln -sf "$SDK_PATH/usr/lib"/* "$SYSROOT/usr/lib/" 2>/dev/null || true
[ -d "$SDK_PATH/System" ] && ln -sf "$SDK_PATH/System" "$SYSROOT/System" 2>/dev/null || true

# Replace libSystem.tbd with Zig's version (includes arm64-macos).
rm -f "$SYSROOT/usr/lib/libSystem.tbd"
cp "$ZIG_LIBC" "$SYSROOT/usr/lib/libSystem.tbd"

echo "sysroot created at $SYSROOT"
