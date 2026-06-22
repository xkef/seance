# Naming: `séance` vs `seance`

Two surfaces, two spellings. Pick by whether a human reads the string or a
machine resolves it.

## Rule

- **Display strings → `séance` / `Séance`.** Anything a person sees: the README
  title, release notes, the macOS `CFBundleDisplayName` / `CFBundleName`, the
  default window title, the dock tile, the menu bar, the Homebrew `desc`, the
  AUR `pkgdesc`, the Debian `Description`.
- **Identifiers → `seance` (ASCII).** Anything a tool resolves: the binary name,
  crate names, `CFBundleIdentifier` (`dev.seance.app`), package names (`brew`,
  `pacman`, `apt`), the config dir (`$XDG_CONFIG_HOME/seance/`), environment
  variables (`SEANCE_*`), the repo slug, and the branch prefix.

The `é` is load-bearing for the brand, so display copy keeps it. But non-ASCII
in a `$PATH`-resolved binary name or a reverse-DNS bundle ID breaks shells and
tooling, so identifiers stay ASCII. The two surfaces never share a string.

## Bundle filename

The macOS bundle on disk is `target/Seance.app` — an ASCII filename with a
capital `S` so `open target/Seance.app` stays typeable. This is the one
sanctioned ASCII `Seance`; the display name inside the bundle
(`CFBundleDisplayName`) is still `Séance`.

## Enforcement

`scripts/check-naming.sh` greps the tracked source for an ASCII capital-`S`
`Seance` outside the `Seance.app` bundle filename and fails if it finds one. CI
runs it on every push and pull request.
