#!/usr/bin/env bash
set -euo pipefail

printf '\033[?25l\033[2J\033[H'

line() {
    printf '%s\n' "$*"
}

swatch_row() {
    local start=$1
    local end=$2
    local i
    for ((i = start; i <= end; i++)); do
        printf '\033[48;5;%sm %3s \033[0m ' "$i" "$i"
    done
    printf '\n'
}

ansi_row() {
    local prefix=$1
    local label=$2
    local i code
    printf '%-14s' "$label"
    for ((i = 0; i <= 7; i++)); do
        code="${prefix}${i}"
        printf '\033[%sm %s%02d \033[0m ' "$code" "$label" "$i"
    done
    printf '\n'
}

truecolor_row() {
    local label=$1
    shift
    local color
    printf '%-14s' "$label"
    for color in "$@"; do
        IFS=, read -r r g b <<<"$color"
        printf '\033[48;2;%s;%s;%sm      \033[0m ' "$r" "$g" "$b"
    done
    printf '\n'
}

line 'seance fidelity fixture'
line 'issue-19: palette, bold-is-bright, min-contrast, gamma/blending'
read -r tty_rows tty_cols < <(stty size)
line "tty size: ${tty_cols}x${tty_rows}"
printf '\n'

line 'palette 0-7'
swatch_row 0 7
line 'palette 8-15'
swatch_row 8 15
printf '\n'

ansi_row '3' 'normal'
ansi_row '9' 'bright'
ansi_row '1;3' 'bold'
printf '\n'

printf '%-14s' 'styles'
printf '\033[36mcyan\033[0m '
printf '\033[2;36mdim-cyan\033[0m '
printf '\033[35mpurple\033[0m '
printf '\033[1;32mbold-green\033[0m '
printf '\033[31merror-red\033[0m\n'
printf '\n'

printf '%-14s' 'near-bg fg'
printf '\033[38;2;60;66;82m#3c4252\033[0m '
printf '\033[38;2;68;75;95m#444b5f\033[0m '
printf '\033[38;2;80;88;110m#50586e\033[0m '
printf '\033[38;2;96;105;129m#606981\033[0m\n'
printf '\n'

truecolor_row 'gray ramp' \
    '24,24,24' '40,40,40' '56,56,56' '72,72,72' '88,88,88' '104,104,104' '120,120,120' '136,136,136'
printf '\n'

line 'text     -> The quick brown fox jumps over the lazy dog 0123456789'
line 'symbols  -> <> [] {} () / \\ | _ - + = ~ * : ; . , @ # % &'
line 'drawing  -> ─ │ ┌ ┐ └ ┘ ├ ┤ ┬ ┴ ┼ ╭ ╮ ╰ ╯ ╳'
line 'powerline->   █ ▓ ▒ ░  ● ○ ◆ ◇'
printf '\n'

printf 'cursor hidden, process sleeping, capture now.\n'
