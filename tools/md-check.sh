#!/usr/bin/env bash
# Markdown / prose checks. CI gates **/*.md against both tools.
# Pass --fix to rewrite files with prettier instead of checking.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ "${1:-}" = "--fix" ]; then
    npx --yes prettier --write "**/*.md"
else
    npx --yes prettier --check "**/*.md"
fi
npx --yes markdownlint-cli2 "**/*.md"
