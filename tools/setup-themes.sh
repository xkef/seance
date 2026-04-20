#!/usr/bin/env bash
# Vendor the Ghostty theme files into vendor/ghostty-themes/.
#
# Rationale:
#   Ghostty does not ship themes inside its source tree — it pulls a tarball
#   of generated files from mbadolato/iTerm2-Color-Schemes at build time
#   (see build.zig.zon's `iterm2_themes` entry). seance consumes those same
#   files by embedding them into the seance-config crate via `include_dir!`.
#   A pinned local clone avoids network at every `cargo build` and gives us
#   a stable hash for reproducible builds.
#
# Usage: tools/setup-themes.sh
#   - Clones on first run, re-clones on commit mismatch, idempotent otherwise.
#   - Run it after bumping ITERM2_COMMIT below, after `cargo clean -p
#     seance-config`, or any time include_dir! misses the themes.

set -euo pipefail

# Pinned iTerm2-Color-Schemes commit. Matches the short SHA suffix on the
# tarball URL in ghostty's build.zig.zon, so seance and Ghostty resolve the
# same theme bytes for the same name. Bump in lockstep with ghostty's pin.
ITERM2_COMMIT="a2c7b60293a81d82767893d89bc7599342c04cbf"
ITERM2_REPO="https://github.com/mbadolato/iTerm2-Color-Schemes.git"

REPO="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$REPO/vendor/iterm2-color-schemes"
DEST="$REPO/vendor/ghostty-themes"
STAMP="$DEST/.seance-commit"

mkdir -p "$REPO/vendor"

need_reclone=0
if [ ! -d "$SRC/.git" ]; then
    need_reclone=1
elif [ ! -f "$STAMP" ] || [ "$(cat "$STAMP" 2>/dev/null)" != "$ITERM2_COMMIT" ]; then
    need_reclone=1
fi

if [ "$need_reclone" = 1 ]; then
    rm -rf "$SRC"
    echo "Cloning iTerm2-Color-Schemes@$ITERM2_COMMIT into vendor/iterm2-color-schemes ..."
    git clone --quiet --filter=blob:none --no-checkout "$ITERM2_REPO" "$SRC"
    git -C "$SRC" -c advice.detachedHead=false checkout --quiet "$ITERM2_COMMIT"

    # Extract just the ghostty/ subdir — that's the only thing we ship.
    rm -rf "$DEST"
    mkdir -p "$DEST"
    cp -a "$SRC/ghostty/." "$DEST/"
    echo "$ITERM2_COMMIT" > "$STAMP"
fi

count=$(find "$DEST" -maxdepth 1 -type f ! -name '.*' | wc -l | tr -d ' ')
echo "vendor/ghostty-themes ready: $count themes at $ITERM2_COMMIT"
