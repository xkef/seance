# Contributing to séance

Day-to-day conventions — commit messages, branch names, code comments, the
markdown gates, and how to pick up an issue — live in [`CLAUDE.md`](CLAUDE.md).
This file covers the parts specific to versioning and cutting a release.

## Versioning

séance follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html):
`MAJOR.MINOR.PATCH`.

- The version is pinned once, in `[workspace.package]` in the root `Cargo.toml`,
  and every crate inherits it with `version.workspace = true`. There is a single
  number to bump.
- While the version is in the `0.x` range the project is pre-1.0: a `MINOR` bump
  (`0.1.0` → `0.2.0`) may carry breaking changes, and `PATCH` bumps stay
  backwards compatible. The first stable API ships as `1.0.0`.
- `seance --version` prints `séance <version> (<git-sha>)`. The version comes
  from `CARGO_PKG_VERSION`; the short SHA is stamped at build time by
  `crates/seance-app/build.rs` and falls back to `unknown` for git-less builds.

## Changelog

User-facing changes go in [`CHANGELOG.md`](CHANGELOG.md), which follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Add entries under the
`## [Unreleased]` heading as you land them, grouped by `Added`, `Changed`,
`Deprecated`, `Removed`, `Fixed`, or `Security`.

## Cutting a release

1. Run `scripts/bump-version.sh <new-version>`. It rewrites the workspace
   version, rolls the `[Unreleased]` entries into a dated section in
   `CHANGELOG.md`, and prints the commit and tag commands. It does not commit or
   push.
2. Review the diff, then commit and tag as printed (`git tag v<new-version>`).
3. Push the tag. CI's `version consistency` job rejects a `v*` tag whose name
   disagrees with the workspace version, so the tree and the tag can never
   drift.
