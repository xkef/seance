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

# Build into a staging dir then atomically rename, so concurrent invocations
# (e.g. several rustc build-script links hitting tools/xcrun at once) never
# observe a half-populated sysroot.
STAGING="$SYSROOT.tmp.$$"
trap 'rm -rf "$STAGING"' EXIT
rm -rf "$STAGING"
mkdir -p "$STAGING/usr/lib" "$STAGING/usr/include"

# Symlink SDK contents.
ln -sf "$SDK_PATH/usr/include"/* "$STAGING/usr/include/" 2>/dev/null || true
ln -sf "$SDK_PATH/usr/lib"/* "$STAGING/usr/lib/" 2>/dev/null || true
[ -d "$SDK_PATH/System" ] && ln -sf "$SDK_PATH/System" "$STAGING/System"

# Symlink SDK-root metadata (SDKSettings.plist, SDKSettings.json,
# Entitlements.plist, Library/, ...). Newer Apple ld reads SDKSettings.plist
# off the sysroot root to determine the SDK version.
for entry in "$SDK_PATH"/*; do
    name="$(basename "$entry")"
    case "$name" in
        usr|System) ;;
        *) ln -sf "$entry" "$STAGING/$name" ;;
    esac
done

# Replace libSystem.tbd with Zig's version (includes arm64-macos).
rm -f "$STAGING/usr/lib/libSystem.tbd"
cp "$ZIG_LIBC" "$STAGING/usr/lib/libSystem.tbd"
printf '%s\n' "$SDK_PATH" > "$STAGING/.sdk-path"

rm -rf "$SYSROOT"
mv "$STAGING" "$SYSROOT"
trap - EXIT

echo "sysroot created at $SYSROOT"
