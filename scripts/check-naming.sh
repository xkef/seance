#!/usr/bin/env bash
set -euo pipefail

# Naming guard. See docs/naming.md.
#
# Display surfaces spell the brand "séance" / "Séance"; identifiers stay ASCII
# "seance". The only sanctioned ASCII capital-S spelling is the macOS bundle
# filename "Seance.app", so flag any other "Seance".

cd "$(dirname "$0")/.."

# Capital-S "Seance" not immediately followed by ".app".
if hits=$(grep -RInP 'Seance(?!\.app)' -- crates tools README.md CLAUDE.md 2>/dev/null); then
  echo "naming: found ASCII 'Seance' where 'Séance' (display) or 'seance'" >&2
  echo "(identifier) is expected. See docs/naming.md." >&2
  echo >&2
  echo "$hits" >&2
  exit 1
fi

echo "naming: ok"
