#!/usr/bin/env bash
set -euo pipefail

# Enforces docs/naming.md: display strings spell the brand "séance"/"Séance",
# while identifiers and on-disk file names stay ASCII "seance". This guards the
# display surfaces against the ASCII "Seance" spelling creeping back in. The
# bundle/artifact file name "Seance.app" is deliberately ASCII and allowed.

cd "$(dirname "$0")/.."

# Capital-S ASCII "Seance" anywhere other than the allowed "Seance.app" file
# name signals a display string that lost its accent.
if hits=$(grep -RIn 'Seance' -- crates tools README.md CLAUDE.md docs \
  | grep -v 'Seance\.app' \
  | grep -v 'tools/check-naming.sh' \
  | grep -v 'docs/naming.md'); then
  echo "naming check failed: 'Seance' should be 'Séance' in display strings" >&2
  echo "(the only allowed ASCII 'Seance' is the 'Seance.app' bundle file name)" >&2
  echo "$hits" >&2
  exit 1
fi

echo "naming check passed"
