#!/usr/bin/env bash
# Fail if HEAD is tagged `v<X.Y.Z>` but the workspace version disagrees.
#
# Run by CI's `version consistency` job and safe to run locally. When HEAD
# carries no exact tag it is a no-op — nothing to reconcile on a branch push.
#
# Usage: scripts/check-version.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# Read the version from [workspace.package] without a full `cargo metadata`
# (which would resolve the git dependency tree). The first `version = "..."`
# line inside that section is the pinned workspace version.
version="$(awk '
  /^\[workspace\.package\]/ { in_pkg = 1; next }
  /^\[/ { in_pkg = 0 }
  in_pkg && /^version[[:space:]]*=/ {
    gsub(/[",]/, "", $3); print $3; exit
  }
' "$ROOT/Cargo.toml")"

if [ -z "$version" ]; then
  echo "error: could not read [workspace.package] version from Cargo.toml" >&2
  exit 2
fi

tag="$(git -C "$ROOT" describe --exact-match --tags HEAD 2>/dev/null || true)"
if [ -z "$tag" ]; then
  echo "HEAD is not tagged; workspace version is $version (nothing to check)."
  exit 0
fi

if [ "$tag" != "v$version" ]; then
  echo "error: tag $tag disagrees with workspace version v$version" >&2
  echo "       bump Cargo.toml or retag before releasing." >&2
  exit 1
fi

echo "tag $tag matches workspace version v$version."
