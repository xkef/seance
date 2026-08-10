# Naming: `séance` vs `seance`

The project brand carries an acute accent — `séance` — but a non-ASCII byte in a
`$PATH`-resolved binary name or a reverse-DNS bundle identifier breaks shells,
package managers, and other tooling. So the surfaces are split: the `é` lives in
human-facing **display strings**, and plain ASCII `seance` is used everywhere a
string is an **identifier** that software parses.

## Rule

**Display strings → `séance` / `Séance`.** Anything a person reads as the
product name:

- README title and release notes.
- macOS `CFBundleName` and `CFBundleDisplayName` (dock tile, menu bar).
- The default window title.
- Homebrew `desc`, AUR `pkgdesc`, Debian `Description`.

**Identifiers → `seance` (ASCII).** Anything software resolves, parses, or keys
on:

- The binary name (`seance`) and `CFBundleExecutable`.
- Crate names (`seance-app`, `seance-vt`, …).
- `CFBundleIdentifier` (`dev.seance.app`).
- Package names (`brew`, `pacman`, `apt`).
- The config directory (`$XDG_CONFIG_HOME/seance/`).
- Environment variables (`SEANCE_*`).
- The repository slug and branch prefixes.

## On-disk file names

App-bundle and artifact file names stay ASCII even though they name a display
artifact, so paths remain typeable and copy-pasteable in a shell. The macOS
bundle is `target/Seance.app` on disk (keeping `open target/Seance.app`
working), while its `CFBundleDisplayName` inside `Info.plist` is `Séance`.

## Enforcement

`tools/check-naming.sh` greps the tracked display surfaces for the ASCII
`Seance` spelling where `Séance` belongs (ignoring the allowed `Seance.app` file
name) and fails if any drift is reintroduced. CI runs it on every push and pull
request.
