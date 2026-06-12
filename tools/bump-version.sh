#!/usr/bin/env bash
# Bump the workspace version, roll CHANGELOG.md, refresh Cargo.lock, and
# print the tag command. Never commits or pushes.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() {
    echo "usage: tools/bump-version.sh <new-version>" >&2
    echo "  e.g. tools/bump-version.sh 0.2.0" >&2
    exit 64
}

[ $# -eq 1 ] || usage
new="$1"
echo "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' || usage

current="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
if [ "$new" = "$current" ]; then
    echo "workspace version is already $current" >&2
    exit 1
fi

# [workspace.package] holds the only top-level `version = "..."` key;
# per-crate manifests inherit via `version.workspace = true`.
tmp="$(mktemp)"
awk -v new="$new" '
    !done && /^version = "/ { print "version = \"" new "\""; done = 1; next }
    { print }
' Cargo.toml >"$tmp"
mv "$tmp" Cargo.toml

today="$(date +%Y-%m-%d)"
tmp="$(mktemp)"
awk -v ver="$new" -v date="$today" '
    { print }
    /^## \[Unreleased\]$/ { print ""; print "## [" ver "] - " date }
' CHANGELOG.md >"$tmp"
mv "$tmp" CHANGELOG.md

cargo update --workspace --quiet

echo "bumped $current -> $new"
echo "next:"
echo "  git commit -am \"chore(release): v$new\""
echo "  git tag v$new"
