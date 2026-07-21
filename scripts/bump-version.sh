#!/usr/bin/env bash
# Bump the workspace version and roll the changelog.
#
# Rewrites `[workspace.package].version` in Cargo.toml, moves the
# `## [Unreleased]` changelog entries into a new dated `## [<version>]`
# section, and prints the commit + tag commands. It never commits, tags,
# or pushes — that stays a deliberate human step.
#
# Usage: scripts/bump-version.sh <major.minor.patch>
set -euo pipefail

new="${1:-}"
case "$new" in
  '' | -h | --help)
    echo "usage: $(basename "$0") <major.minor.patch>"
    [ -z "$new" ] && exit 2 || exit 0
    ;;
esac

if ! printf '%s' "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$'; then
  echo "error: '$new' is not a valid semantic version" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

current="$(awk '
  /^\[workspace\.package\]/ { in_pkg = 1; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && /^version[[:space:]]*=/ { gsub(/[",]/, "", $3); print $3; exit }
' Cargo.toml)"

if [ -z "$current" ]; then
  echo "error: could not read current workspace version" >&2
  exit 2
fi

if [ "$current" = "$new" ]; then
  echo "error: workspace version is already $new" >&2
  exit 2
fi

# Rewrite only the version line inside [workspace.package].
tmp="$(mktemp)"
awk -v new="$new" '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && !done && /^version[[:space:]]*=/ {
    print "version = \"" new "\""; done = 1; next
  }
  { print }
' Cargo.toml > "$tmp"
mv "$tmp" Cargo.toml

# Roll [Unreleased] into a dated release section. The entries that followed
# [Unreleased] fall under the new [<new>] header; [Unreleased] is left empty.
today="$(date +%Y-%m-%d)"
tmp="$(mktemp)"
awk -v new="$new" -v date="$today" '
  /^## \[Unreleased\]/ {
    print
    print ""
    print "## [" new "] - " date
    next
  }
  { print }
' CHANGELOG.md > "$tmp"
mv "$tmp" CHANGELOG.md

echo "Bumped workspace version $current -> $new and rolled CHANGELOG.md."
echo
echo "Review the changes, then:"
echo "  git add Cargo.toml CHANGELOG.md"
echo "  git commit -m \"chore(release): v$new\""
echo "  git tag v$new"
echo "  git push && git push origin v$new"
