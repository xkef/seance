# seance

GPU-rendered terminal built on `libghostty-vt` (the Rust bindings from
[libghostty-rs](https://github.com/Uzaaft/libghostty-rs)) and `wgpu`.
macOS-first; Linux is a target but untested as of this writing.

## Crate layout

| Crate                | Role                                                                                                |
| -------------------- | --------------------------------------------------------------------------------------------------- |
| `seance-app`         | winit event loop, `Window`, top-level `App`. Drives PTY polling and redraw dispatch.                |
| `seance-vt`          | VT adapter around `libghostty-vt` — terminal state, render-state iteration, kitty-graphics adapter. |
| `seance-render`      | wgpu pipelines, glyph atlas (cosmic-text + swash), image renderer.                                  |
| `seance-input`       | winit key/mouse → VT escape sequences, Cmd shortcut dispatch.                                       |
| `seance-protocol`    | Wire-level mux protocol: owned snapshot/delta types, postcard envelopes, `Transport` trait.         |
| `seance-frame`       | Render-facing frame traits (`FrameSource` + visitors) bridging protocol snapshots to the renderer.  |
| `seance-mux-client`  | Client-side mux: `Domain` seam, `MuxClient`/`PaneView` materialization, link detection.             |
| `seance-mux-server`  | Server-side mux: `LocalDomain` over seance-vt, the `serve` protocol-dispatch loop.                  |
| `seance-config`      | `config.toml` schema + loading, hot-reload `ConfigDiff`, theme resolution.                          |
| `seance-render-test` | Layered renderer test harness: headless VT, ASCII frame dumps, snapshot tests.                      |
| `seance-bench`       | Frame-time bench harness: CPU stopwatch + headless GPU timing.                                      |

Canonical architecture reference: **`docs/architecture.md`**. Read that before
touching the renderer or VT layer.

## First-time setup

```sh
tools/setup.sh               # ghostty-src + themes + macOS sysroot
```

Re-run it after `cargo clean` or after bumping `libghostty-vt` in `Cargo.toml`.

## Everyday commands

```sh
tools/run.sh                 # setup + cargo run
cargo check --workspace
cargo fmt --all
cargo fmt-check              # alias: fmt --all -- --check
cargo lint                   # alias: clippy --workspace --all-targets -- -D warnings
cargo test --workspace
tools/ci.sh                  # every CI gate: fmt, clippy, tests, markdown

# Markdown / prose checks (CI runs both against **/*.md):
tools/md-check.sh            # pass --fix to rewrite with prettier
```

Cargo aliases (`lint`, `fmt-check`) are defined in `.cargo/config.toml`.

CI gates every push on `prettier --check` and `markdownlint-cli2`. Any change
that touches a `.md` file (including `CLAUDE.md`, `README.md`, and anything
under `docs/`) MUST pass both locally before you commit — run
`npx --yes prettier --write <files>` to auto-fix wrap/indent issues, and re-run
`--check` until it is clean. Do not push first and let CI catch it.

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

The same rule kills time-bound narration: "Today every X is recomputed", "We
currently do Y", "Without this change…". The moment the PR merges, those
sentences describe a state that no longer exists. Describe the current invariant
in tense-neutral terms — "this cache memoizes X", "cells are shaped on miss" —
and put the before-state in the commit body where it belongs.

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
   If the change touches any `.md` file, also run
   `npx --yes prettier --check "**/*.md"` and
   `npx --yes markdownlint-cli2 "**/*.md"` and resolve every diagnostic before
   pushing — these are the same commands CI runs, so a green local run is the
   bar.

## GitHub issue, PR, and comment bodies

When creating or editing GitHub issue bodies, PR bodies, or comments, do not
apply the 72-column commit-message wrapping rule to prose. Use one physical line
per paragraph and let GitHub/GFM wrap it. Hard-wrap only content that should not
reflow: fenced code, logs, quoted output, tables, and ASCII diagrams. Titles
stay single-line and short, ideally around 50–70 characters.

When fixing existing GitHub body wrapping, change wrapping only unless the user
explicitly asks for content edits. Do not add a comment just to announce a
wrapping-only body edit. There is no supported way to suppress GitHub's edit
metadata/history, so do not promise a trace-free edit.

## Commit messages

Use Conventional Commits (<https://www.conventionalcommits.org/>) for every
commit and PR title: `type(scope): summary`, with `type` drawn from `feat`,
`fix`, `refactor`, `perf`, `docs`, `test`, `style`, `chore`, `build`, `ci`. Keep
the subject line under 72 characters. Wrap commit bodies at 72 columns. Put the
why (and any design narration that would otherwise leak into code comments) in
the body.

## Pull requests

PRs opened by Claude MUST use a Conventional Commit title. PR bodies explain the
same why as commit bodies, but GitHub renders them with GFM, so do not manually
wrap prose.

### Subject (PR title / commit subject)

- Conventional Commits: `type(scope): summary`. Scope is optional.
- Allowed types: `feat`, `fix`, `refactor`, `docs`, `test`, `perf`, `style`,
  `chore`, `build`, `ci`.
- Imperative mood, no trailing period, lowercase after the type.
- Aim for ≤50 characters; 72 is the hard limit (GitHub truncates beyond it).

### Body (PR body)

- Use one physical line per prose paragraph; do not hard-wrap at 72 columns.
- Explain _why_, not _what_. The diff already shows what changed.
- Separate paragraphs with blank lines.
- Footers (last block): `Closes #<issue>` is REQUIRED whenever the PR addresses
  one or more issues — list one per line so GitHub auto-closes them on merge.
  Use `Refs: #<issue>` only for issues the PR references but does not fully
  resolve. `Breaking-Change:` is optional.
- If the PR genuinely addresses no tracked issue, say so explicitly in the body
  rather than omitting the footer silently.

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
