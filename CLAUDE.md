# seance

GPU-rendered terminal built on `libghostty-vt` (the Rust bindings from
[libghostty-rs](https://github.com/Uzaaft/libghostty-rs)) and `wgpu`.
macOS-first; Linux is a target but untested as of this writing.

## Crate layout

| Crate           | Role                                                                                                |
| --------------- | --------------------------------------------------------------------------------------------------- |
| `seance-app`    | winit event loop, `Window`, top-level `App`. Drives PTY polling and redraw dispatch.                |
| `seance-vt`     | VT adapter around `libghostty-vt` — terminal state, render-state iteration, kitty-graphics adapter. |
| `seance-render` | wgpu pipelines, glyph atlas (cosmic-text + swash), image renderer.                                  |
| `seance-input`  | winit key/mouse → VT escape sequences, Cmd shortcut dispatch.                                       |

Canonical architecture reference: **`docs/architecture.md`**. Read that before
touching the renderer or VT layer.

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

## Code comments

Default to writing no comments. Do not add doc comments that restate what an
identifier + type already communicate — `pub theme: Theme` or
`CONFIG_FILENAME = "config.toml"` do not need a one-line summary above them.
Reserve comments for things a reader cannot infer from the signature: hidden
invariants (e.g. "alpha always 0xff"), `Option` semantics (what `None` means
here), surprising behavior, cross-file references, or a workaround tied to a
specific bug.

Design-decision narration ("this crate holds no X", "replaces Y") belongs in the
commit message, not the code — it rots as the codebase evolves.

## Working on issues

Epics are tracked on GitHub under the `epic` label (M1–M9). Every non-epic issue
must be attached to its parent epic as a sub-issue — when filing a new issue,
identify the epic it belongs under and link it. If no existing epic fits, open a
new epic first rather than creating an orphan issue.

When picking up a sub-issue:

1. Read the parent epic for context, plus `docs/architecture.md` for the section
   it touches.
2. Reference the specific files/modules you intend to change in the PR
   description.
3. Keep changes scoped to the sub-issue — do not batch unrelated cleanups.
4. Run `cargo fmt`, `cargo clippy`, and the relevant tests before opening a PR.

## Commit messages

Use Conventional Commits (<https://www.conventionalcommits.org/>) for every
commit and PR title: `type(scope): summary`, with `type` drawn from `feat`,
`fix`, `refactor`, `perf`, `docs`, `test`, `style`, `chore`, `build`, `ci`. Keep
the subject line under 72 characters. Put the why (and any design narration that
would otherwise leak into code comments) in the body.

## Pull requests

PRs opened by Claude MUST follow the same format as commits. The PR title is the
subject; the PR body is the commit body. Apply these rules to both.

### Subject (PR title / commit subject)

- Conventional Commits: `type(scope): summary`. Scope is optional.
- Allowed types: `feat`, `fix`, `refactor`, `docs`, `test`, `perf`, `style`,
  `chore`, `build`, `ci`.
- Imperative mood, no trailing period, lowercase after the type.
- Aim for ≤50 characters; 72 is the hard limit (GitHub truncates beyond it).

### Body (PR body / commit body)

- Wrap every line at 72 columns.
- Explain _why_, not _what_. The diff already shows what changed.
- Separate the subject from the body with a blank line.
- Footers (optional, last block): `Breaking-Change:`, `Refs: #<issue>`.

### Forbidden in PRs and commits

- No `Co-Authored-By:` lines.
- No `Generated with` / `Created by Claude` / tool-attribution footers.
- No emoji-prefixed footers (e.g. 🤖) or marketing taglines.
- No links back to the agent session, chat URL, or any
  `https://claude.ai/code/...` reference.
- No HTML comments, no `<details>` collapsibles, no checkbox "test plan"
  templates unless the user explicitly asks for one.

When using the GitHub MCP tools to open a PR, pass the title and body verbatim —
do not append any auto-generated trailer.

## Branches

Branch names follow Conventional Commits, mirroring the commit `type` and
optional `scope`: `<type>/<short-kebab-summary>` or
`<type>-<scope>/<short-kebab-summary>`.

- `type` is one of the Conventional Commit types listed above.
- The summary is lowercase, kebab-cased, and describes the change — not the
  author or the agent.
- No `claude/` (or other agent/author) prefix.
- No random hash, timestamp, or session suffix at the end.
- Keep it short; aim for ≤40 characters total.

Examples: `feat/dirty-row-tracking`, `fix-cursor/honor-decscusr`,
`docs/architecture-vt-section`, `refactor/split-platform-modules`.

Never push directly to `main`.
