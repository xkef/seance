# Naming: `séance` vs `seance`

The project name carries a load-bearing acute accent. The `é` is part of the
brand, but non-ASCII bytes in a `$PATH`-resolved binary name or a reverse-DNS
bundle identifier break shells and tooling. So the two surfaces are split:
accented for human-facing display, ASCII for machine-facing identifiers.

## Display strings → `séance` / `Séance`

Anything a person reads as a name:

- README title and release notes
- macOS `CFBundleName` / `CFBundleDisplayName`, dock tile, menu bar
- default window title
- Homebrew `desc`, AUR `pkgdesc`, Debian `Description`

## Identifiers → `seance` (ASCII)

Anything a machine parses, resolves, or matches:

- binary name and crate names
- `CFBundleIdentifier` (`dev.seance.app`) and `CFBundleExecutable` (`seance`)
- package names (`brew`, `pacman`, `apt`)
- config dir (`$XDG_CONFIG_HOME/seance/`) and env vars (`SEANCE_*`)
- `TERM_PROGRAM` value (`seance`), repo slug, branch prefixes

The macOS bundle directory on disk stays `target/Seance.app` (ASCII filename),
so `open target/Seance.app` remains typeable even though the display name inside
the bundle is `Séance`.

## Enforcement

`tools/check-naming.sh` greps the tree for the capital-S ASCII spelling `Seance`
and fails on any hit outside the `Seance.app` bundle name. It runs as the
`naming` job in CI and is part of `tools/ci.sh`.
