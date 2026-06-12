# Contributing

Development setup, everyday commands, and commit/PR/branch conventions live in
[CLAUDE.md](CLAUDE.md). `tools/ci.sh` runs every CI gate locally; a green run
there is the bar for pushing.

## Versioning

seance follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html):
`MAJOR.MINOR.PATCH`. While the project is pre-1.0 (`0.x`), minor versions may
break configuration, protocol, or API compatibility; patch versions are fixes
only.

The version is defined exactly once, in `[workspace.package]` in the root
`Cargo.toml`; every crate inherits it via `version.workspace = true`. Release
tags are `v<version>` (e.g. `v0.1.0`), and CI rejects any tag that does not
match the workspace version.

User-visible changes are recorded in [CHANGELOG.md](CHANGELOG.md) under
`[Unreleased]`, in [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
format, as part of the PR that makes them.

## Releasing

1. Run `tools/bump-version.sh <new-version>`. It updates the workspace version,
   refreshes `Cargo.lock`, moves the `[Unreleased]` entries in `CHANGELOG.md`
   into a new dated `[<new-version>]` section, and prints the tag command. It
   never pushes.
2. Review the diff and commit it as `chore(release): v<new-version>`.
3. Tag the release commit with the printed command and push the tag.
