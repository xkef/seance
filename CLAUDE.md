# seance

GPU-rendered terminal built on `libghostty-vt` (the Rust bindings from
[libghostty-rs](https://github.com/Uzaaft/libghostty-rs)) and `wgpu`.
macOS-first; Linux is a target but untested as of this writing.

## Crate layout

| Crate | Role |
|---|---|
| `seance-app` | winit event loop, `Window`, top-level `App`. Drives PTY polling and redraw dispatch. |
| `seance-vt` | VT adapter around `libghostty-vt` — terminal state, render-state iteration, kitty-graphics adapter. |
| `seance-render` | wgpu pipelines, glyph atlas (cosmic-text + swash), image renderer. |
| `seance-input` | winit key/mouse → VT escape sequences, Cmd shortcut dispatch. |

Canonical architecture reference: **`docs/architecture.md`**. Read that
before touching the renderer or VT layer.

## First-time setup

```sh
tools/setup-sysroot.sh       # macOS 26 SDK overlay for Zig's arm64 linker
tools/setup-ghostty-src.sh   # clones + patches vendored ghostty-src
```

Re-run `setup-ghostty-src.sh` after `cargo clean` or after bumping
`libghostty-vt` in `Cargo.toml`.

## Everyday commands

```sh
tools/run.sh                 # setup-ghostty-src + cargo run
cargo check --workspace
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Working on issues

Epics are tracked on GitHub under the `epic` label (M1–M7). Each epic
decomposes into sub-issues sized for roughly one agent session. When
picking up a sub-issue:

1. Read the parent epic for context, plus `docs/architecture.md` for
   the section it touches.
2. Reference the specific files/modules you intend to change in the PR
   description.
3. Keep changes scoped to the sub-issue — do not batch unrelated
   cleanups.
4. Run `cargo fmt`, `cargo clippy`, and the relevant tests before
   opening a PR.

## Branches

Development branches follow `claude/<short-slug>-<suffix>`. Never push
directly to `main`.
