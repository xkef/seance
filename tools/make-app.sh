#!/usr/bin/env bash
set -euo pipefail

# Builds a minimal Seance.app bundle at target/Seance.app so macOS launchd
# treats seance as a proper foreground app (real activation policy, key
# focus, menubar). For debugging focus/activation issues on unbundled runs.

cd "$(dirname "$0")/.."
ROOT="$PWD"

PROFILE="${1:-release}"
BIN="$ROOT/target/$PROFILE/seance"
DYLIB="$(find "$ROOT/target/$PROFILE/build" -path '*/ghostty-install/lib/libghostty-vt.dylib' -print -quit)"

echo "building $PROFILE binary..."
cargo build ${PROFILE:+--profile=$PROFILE} 2>/dev/null || cargo build --release
[[ -x "$BIN" ]] || { echo "binary missing: $BIN" >&2; exit 1; }
[[ -f "$DYLIB" ]] || { echo "dylib missing under target/$PROFILE/build" >&2; exit 1; }

APP="$ROOT/target/Seance.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks"

cp "$BIN" "$APP/Contents/MacOS/seance"
cp "$DYLIB" "$APP/Contents/Frameworks/libghostty-vt.dylib"

install_name_tool -add_rpath "@executable_path/../Frameworks" "$APP/Contents/MacOS/seance"

cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>dev.seance.app</string>
  <key>CFBundleName</key><string>Seance</string>
  <key>CFBundleDisplayName</key><string>Seance</string>
  <key>CFBundleExecutable</key><string>seance</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleShortVersionString</key><string>0.1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSPrincipalClass</key><string>NSApplication</string>
</dict></plist>
PLIST

codesign --force --sign - "$APP" >/dev/null

echo "built: $APP"
echo "run:   open \"$APP\""
