#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/../.."
ROOT="$PWD"

usage() {
    cat <<'EOF'
Usage: bash tools/fidelity/capture_pair.sh

Build seance, launch Ghostty and seance sequentially with matched configs,
capture both windows, and write screenshots plus diff artifacts.

Override settings with FIDELITY_* environment variables. See
tools/fidelity/README.md for the full list.
EOF
}

if [[ "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "fidelity: macOS only" >&2
    exit 1
fi

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "fidelity: missing command: $1" >&2
        exit 1
    }
}

for cmd in open osascript pgrep python3 screencapture xcrun; do
    require_cmd "$cmd"
done

PROFILE="${FIDELITY_PROFILE:-release}"
OUT_DIR="${FIDELITY_OUT_DIR:-tools/fidelity/artifacts/latest}"
THEME="${FIDELITY_THEME:-Catppuccin Frappe}"
FONT_FAMILY="${FIDELITY_FONT_FAMILY:-JetBrainsMono Nerd Font}"
FONT_SIZE="${FIDELITY_FONT_SIZE:-14}"
MIN_CONTRAST="${FIDELITY_MIN_CONTRAST:-1.1}"
ADJUST_CELL_HEIGHT="${FIDELITY_ADJUST_CELL_HEIGHT:-20%}"
BOLD_IS_BRIGHT="${FIDELITY_BOLD_IS_BRIGHT:-false}"
PADDING_X="${FIDELITY_PADDING_X:-12}"
PADDING_Y="${FIDELITY_PADDING_Y:-0}"
DECORATION="${FIDELITY_DECORATION:-false}"
BACKGROUND_OPACITY="${FIDELITY_BACKGROUND_OPACITY:-1.0}"
CURSOR_STYLE="${FIDELITY_CURSOR_STYLE:-block}"
CURSOR_BLINK="${FIDELITY_CURSOR_BLINK:-false}"
BOUNDS="${FIDELITY_BOUNDS:-120,120,1100,760}"
SETTLE_SECONDS="${FIDELITY_SETTLE_SECONDS:-1.25}"
FIXTURE="${FIDELITY_FIXTURE:-tools/fidelity/fixture.sh}"
GHOSTTY_APP="${FIDELITY_GHOSTTY_APP:-/Applications/Ghostty.app}"
SEANCE_APP="${FIDELITY_SEANCE_APP:-$ROOT/target/Seance.app}"
SEANCE_BIN="${FIDELITY_SEANCE_BIN:-$SEANCE_APP/Contents/MacOS/seance}"

case "$OUT_DIR" in
    /*) ;;
    *) OUT_DIR="$ROOT/$OUT_DIR" ;;
esac
case "$FIXTURE" in
    /*) ;;
    *) FIXTURE="$ROOT/$FIXTURE" ;;
esac
case "$GHOSTTY_APP" in
    /*) ;;
    *) GHOSTTY_APP="$ROOT/$GHOSTTY_APP" ;;
esac
case "$SEANCE_APP" in
    /*) ;;
    *) SEANCE_APP="$ROOT/$SEANCE_APP" ;;
esac
case "$SEANCE_BIN" in
    /*) ;;
    *) SEANCE_BIN="$ROOT/$SEANCE_BIN" ;;
esac

[[ -f "$FIXTURE" ]] || {
    echo "fidelity: fixture missing: $FIXTURE" >&2
    exit 1
}
[[ -d "$GHOSTTY_APP" ]] || {
    echo "fidelity: Ghostty.app missing: $GHOSTTY_APP" >&2
    exit 1
}

IFS=, read -r WIN_X WIN_Y WIN_W WIN_H <<<"$BOUNDS"
for value in "$WIN_X" "$WIN_Y" "$WIN_W" "$WIN_H"; do
    [[ "$value" =~ ^[0-9]+$ ]] || {
        echo "fidelity: FIDELITY_BOUNDS must be x,y,width,height" >&2
        exit 1
    }
done

mkdir -p "$ROOT/tools/fidelity/artifacts" "$OUT_DIR"
TMP_DIR="$(mktemp -d "$ROOT/tools/fidelity/artifacts/tmp.XXXXXX")"
WINDOW_INFO_BIN="$TMP_DIR/window-info"
PIDS=()

cleanup() {
    local rc=$?
    trap - EXIT INT TERM
    for pid in "${PIDS[@]:-}"; do
        if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
            sleep 0.2
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
    rm -rf "$TMP_DIR"
    exit "$rc"
}
trap cleanup EXIT INT TERM

new_pid_for_name() {
    local name=$1
    local baseline=$2
    local end=$((SECONDS + 20))
    local pid

    while ((SECONDS < end)); do
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            if ! grep -qx -- "$pid" <<<"$baseline"; then
                printf '%s\n' "$pid"
                return 0
            fi
        done < <(pgrep -x "$name" || true)
        sleep 0.2
    done
    return 1
}

wait_for_window_json() {
    local pid=$1
    local timeout=${2:-20}
    local end=$((SECONDS + timeout))
    local json

    while ((SECONDS < end)); do
        if json="$("$WINDOW_INFO_BIN" "$pid" 2>/dev/null)"; then
            printf '%s\n' "$json"
            return 0
        fi
        sleep 0.2
    done
    return 1
}

set_window_bounds() {
    local pid=$1
    local x=$2
    local y=$3
    local w=$4
    local h=$5

    osascript - "$pid" "$x" "$y" "$w" "$h" <<'APPLESCRIPT' >/dev/null
on run argv
    set pidValue to (item 1 of argv) as integer
    set xValue to (item 2 of argv) as integer
    set yValue to (item 3 of argv) as integer
    set wValue to (item 4 of argv) as integer
    set hValue to (item 5 of argv) as integer

    tell application "System Events"
        set targetProcess to first application process whose unix id is pidValue
        set frontmost of targetProcess to true
        repeat 100 times
            if (count of windows of targetProcess) > 0 then
                exit repeat
            end if
            delay 0.1
        end repeat
        tell window 1 of targetProcess
            set position to {xValue, yValue}
            set size to {wValue, hValue}
        end tell
    end tell
end run
APPLESCRIPT
}

window_id_from_json() {
    python3 -c 'import json,sys; print(json.load(sys.stdin)["window_id"])'
}

capture_window() {
    local pid=$1
    local output_png=$2
    local name=$3
    local json
    local window_id

    wait_for_window_json "$pid" 20 >/dev/null || {
        echo "fidelity: $name window did not appear" >&2
        exit 1
    }
    set_window_bounds "$pid" "$WIN_X" "$WIN_Y" "$WIN_W" "$WIN_H"
    sleep "$SETTLE_SECONDS"
    json="$(wait_for_window_json "$pid" 10)" || {
        echo "fidelity: failed to read $name window info" >&2
        exit 1
    }
    printf '%s\n' "$json" >"$OUT_DIR/$name.window.json"
    window_id="$(printf '%s' "$json" | window_id_from_json)"
    screencapture -x -o -l "$window_id" "$output_png"
}

write_wrapper() {
    cat >"$TMP_DIR/driver.sh" <<EOF
#!/usr/bin/env bash
set -euo pipefail
cd "$ROOT"
export TERM=xterm-256color
export COLORTERM=truecolor
/bin/bash "$FIXTURE"
exec /bin/sleep 600
EOF
    chmod +x "$TMP_DIR/driver.sh"
}

write_ghostty_config() {
    cat >"$TMP_DIR/ghostty.config" <<EOF
font-family = $FONT_FAMILY
font-size = $FONT_SIZE
adjust-cell-height = $ADJUST_CELL_HEIGHT
bold-is-bright = $BOLD_IS_BRIGHT
minimum-contrast = $MIN_CONTRAST
theme = $THEME
window-padding-x = $PADDING_X
window-padding-y = $PADDING_Y
window-decoration = $DECORATION
background-opacity = $BACKGROUND_OPACITY
cursor-style = $CURSOR_STYLE
cursor-style-blink = $CURSOR_BLINK
confirm-close-surface = false
EOF
}

write_seance_config() {
    mkdir -p "$TMP_DIR/xdg/seance"
    cat >"$TMP_DIR/xdg/seance/config.toml" <<EOF
theme = "$THEME"

[font]
family = "$FONT_FAMILY"
size = $FONT_SIZE
adjust_cell_height = "$ADJUST_CELL_HEIGHT"
min_contrast = $MIN_CONTRAST

[window]
padding_x = $PADDING_X
padding_y = $PADDING_Y
decoration = $DECORATION
background_opacity = $BACKGROUND_OPACITY

[cursor]
style = "$CURSOR_STYLE"
blink = $CURSOR_BLINK

[clipboard]
read = true
write = true
paste_protection = true
copy_on_select = false

[scrollback]
limit = 50000

[mouse]
hide_while_typing = true
EOF
}

write_capture_env() {
    {
        printf 'FIDELITY_PROFILE=%q\n' "$PROFILE"
        printf 'FIDELITY_THEME=%q\n' "$THEME"
        printf 'FIDELITY_FONT_FAMILY=%q\n' "$FONT_FAMILY"
        printf 'FIDELITY_FONT_SIZE=%q\n' "$FONT_SIZE"
        printf 'FIDELITY_MIN_CONTRAST=%q\n' "$MIN_CONTRAST"
        printf 'FIDELITY_ADJUST_CELL_HEIGHT=%q\n' "$ADJUST_CELL_HEIGHT"
        printf 'FIDELITY_BOLD_IS_BRIGHT=%q\n' "$BOLD_IS_BRIGHT"
        printf 'FIDELITY_PADDING_X=%q\n' "$PADDING_X"
        printf 'FIDELITY_PADDING_Y=%q\n' "$PADDING_Y"
        printf 'FIDELITY_DECORATION=%q\n' "$DECORATION"
        printf 'FIDELITY_BACKGROUND_OPACITY=%q\n' "$BACKGROUND_OPACITY"
        printf 'FIDELITY_CURSOR_STYLE=%q\n' "$CURSOR_STYLE"
        printf 'FIDELITY_CURSOR_BLINK=%q\n' "$CURSOR_BLINK"
        printf 'FIDELITY_BOUNDS=%q\n' "$BOUNDS"
        printf 'FIDELITY_SETTLE_SECONDS=%q\n' "$SETTLE_SECONDS"
        printf 'FIDELITY_FIXTURE=%q\n' "$FIXTURE"
        printf 'FIDELITY_GHOSTTY_APP=%q\n' "$GHOSTTY_APP"
        printf 'FIDELITY_SEANCE_APP=%q\n' "$SEANCE_APP"
        printf 'FIDELITY_SEANCE_BIN=%q\n' "$SEANCE_BIN"
    } >"$OUT_DIR/capture.env"
}

build_window_info_helper() {
    xcrun swiftc "$ROOT/tools/fidelity/window_info.swift" -o "$WINDOW_INFO_BIN" >/dev/null 2>/dev/null
}

build_seance_bundle() {
    bash tools/setup-ghostty-src.sh >/dev/null
    bash tools/setup-themes.sh >/dev/null
    bash tools/setup-sysroot.sh >/dev/null
    bash tools/make-app.sh "$PROFILE" >/dev/null
    [[ -x "$SEANCE_BIN" ]] || {
        echo "fidelity: seance binary missing after build: $SEANCE_BIN" >&2
        exit 1
    }
}

launch_ghostty() {
    local baseline
    baseline="$(pgrep -x Ghostty || true)"
    open -na "$GHOSTTY_APP" --args --config-file="$TMP_DIR/ghostty.config" -e "$TMP_DIR/driver.sh"
    local pid
    pid="$(new_pid_for_name Ghostty "$baseline")" || {
        echo "fidelity: failed to find new Ghostty pid" >&2
        exit 1
    }
    PIDS+=("$pid")
    printf '%s\n' "$pid"
}

launch_seance() {
    XDG_CONFIG_HOME="$TMP_DIR/xdg" SHELL="$TMP_DIR/driver.sh" "$SEANCE_BIN" \
        >"$OUT_DIR/seance.stdout.log" 2>"$OUT_DIR/seance.stderr.log" &
    local pid=$!
    PIDS+=("$pid")
    printf '%s\n' "$pid"
}

rm -f \
    "$OUT_DIR/ghostty.png" \
    "$OUT_DIR/seance.png" \
    "$OUT_DIR/diff.png" \
    "$OUT_DIR/metrics.json" \
    "$OUT_DIR/ghostty.config" \
    "$OUT_DIR/seance.toml" \
    "$OUT_DIR/capture.env" \
    "$OUT_DIR/ghostty.window.json" \
    "$OUT_DIR/seance.window.json" \
    "$OUT_DIR/seance.stdout.log" \
    "$OUT_DIR/seance.stderr.log"

write_wrapper
write_ghostty_config
write_seance_config
write_capture_env
build_window_info_helper
build_seance_bundle
cp "$TMP_DIR/ghostty.config" "$OUT_DIR/ghostty.config"
cp "$TMP_DIR/xdg/seance/config.toml" "$OUT_DIR/seance.toml"

GHOSTTY_PID="$(launch_ghostty)"
capture_window "$GHOSTTY_PID" "$OUT_DIR/ghostty.png" ghostty
kill "$GHOSTTY_PID" 2>/dev/null || true
sleep 0.5

SEANCE_PID="$(launch_seance)"
capture_window "$SEANCE_PID" "$OUT_DIR/seance.png" seance
kill "$SEANCE_PID" 2>/dev/null || true
sleep 0.5

python3 tools/fidelity/compare.py \
    --reference "$OUT_DIR/ghostty.png" \
    --candidate "$OUT_DIR/seance.png" \
    --diff "$OUT_DIR/diff.png" \
    --metrics "$OUT_DIR/metrics.json"

printf 'wrote %s\n' "$OUT_DIR"
printf '  %s\n' "$OUT_DIR/ghostty.png"
printf '  %s\n' "$OUT_DIR/seance.png"
printf '  %s\n' "$OUT_DIR/diff.png"
printf '  %s\n' "$OUT_DIR/metrics.json"
