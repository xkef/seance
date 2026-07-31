#!/usr/bin/env bash
# Enforce the naming split from docs/naming.md.
#
# Display strings use the accented "Séance"/"séance"; identifiers use the ASCII
# lowercase "seance". The one permitted capital-S ASCII spelling is the macOS
# bundle filename "Seance.app", kept ASCII so `open target/Seance.app` stays
# typeable. Any other "Seance" is drift — usually a display string that lost
# its accent.
set -euo pipefail
cd "$(dirname "$0")/.."

# Strip the allowed "Seance.app" token first, then flag any surviving "Seance".
# This file names the forbidden spelling in its own docs, so exclude it.
hits=$(grep -RIn 'Seance' \
  --include='*.rs' --include='*.sh' --include='*.md' --include='*.toml' \
  --exclude='check-naming.sh' \
  crates tools README.md CLAUDE.md \
  | sed 's/Seance\.app//g' \
  | grep 'Seance' || true)

if [[ -n "$hits" ]]; then
  echo "check-naming: forbidden \"Seance\" outside the \"Seance.app\" bundle name:" >&2
  echo "$hits" >&2
  echo >&2
  echo "Use display \"Séance\" or ASCII identifier \"seance\" (see docs/naming.md)." >&2
  exit 1
fi

echo "check-naming: ok"
