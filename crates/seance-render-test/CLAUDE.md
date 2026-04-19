# seance-render-test

Layered test harness for the seance renderer. Phase A: Layer 1 (pure
logic / VT behavior) + Layer 4 (frame-assembly snapshots). Future
phases add font raster (L2), shaper (L3), headless GPU (L5), PNG
goldens (L6), property/fuzz (L8), benches (L9).

## How to use

Two kinds of tests live here:

- **`tests/layer1_logic.rs`** — plain `#[test]`. Asserts observable VT
  behavior through `HeadlessTerminal` (cursor moves, modes, CSI
  handling). No fonts, no fixtures, no snapshots. A failure names the
  specific invariant that regressed.
- **`tests/layer4_frame.rs`** — `insta::assert_snapshot!`. Feeds a VT
  fixture, dumps the rendered grid as text + per-cell annotations.
  When behavior changes deliberately, bless with `just snap-review`
  (or `cargo insta review -p seance-render-test`). When behavior
  changes unintentionally, **read the diff in the grid box** to see
  what moved — that's the failure diagnosis.

## Layer → file → failure-action map

| Layer | Test file                  | Failure action                                                                      |
| ----- | -------------------------- | ----------------------------------------------------------------------------------- |
| L1    | `tests/layer1_logic.rs`    | Inspect the assertion. Behavior comes from `seance-vt` + `libghostty-vt` internals. |
| L4    | `tests/layer4_frame.rs`    | Open the diff. The grid box in the snapshot names the wrong cells visually.         |
| L2    | _(Phase B)_                | Font raster hash drifted — `rustybuzz`/`swash` bumped? Re-bless after review.       |
| L3    | _(Phase B)_                | Shaper cluster boundary changed — open the cluster dump, trace to cosmic-text.      |
| L5    | _(Phase B)_                | Headless wgpu structural assertion failed — check `render_to_rgba` + cell_bg/text.  |
| L6    | _(Phase E, opt-in)_        | Pixel-level regression — open `failures/<test>_diff.png`.                           |
| L8    | _(Phase D)_                | Proptest found a case; reproduce with the printed seed.                             |

## Do not modify snapshots to make tests pass

A `.snap` file is the contract. If a test fails:

1. Read the diff. The grid box and the `cells:` block show what the
   renderer produced vs what it should produce.
2. If the new output is **wrong** — fix the renderer, not the snapshot.
3. If the new output is **correct** (deliberate behavioral change) —
   re-bless with `just snap-review`.

Never accept a snapshot without reading it first.

## Test-support seams (`#[doc(hidden)]`)

The harness reaches into two crates through `#[doc(hidden)]` modules:

- **`seance_vt::test_support::HeadlessTerminal`** — PTY-less VT
  constructor. Not a stable API; do not widen without the user's
  say-so.
- _(Phase B)_ `seance_render::test_support` — not yet landed.

These are `#[doc(hidden)]` on purpose: consumers outside this harness
must not depend on them.

## Commands

```sh
just test                 # runs cargo nextest on the full workspace
just test-render          # runs just this crate
just snap-review          # cargo insta review for this crate
```

To bless snapshots non-interactively:

```sh
INSTA_UPDATE=always cargo test -p seance-render-test
```

## Fixtures

`fixtures/vt_streams/` holds raw VT byte streams. Current catalog:

| File              | Bytes | Purpose                                         |
| ----------------- | ----- | ----------------------------------------------- |
| `empty.bin`       | 0     | Cold grid, no input                             |
| `hello_world.bin` | 12    | Plain ASCII; LF (no CR) cursor behavior         |
| `ansi_colors.bin` | 57    | SGR 16-color + truecolor; palette vs passthrough |
| `box_drawing.bin` | 36    | Unicode box chars (square-corner) with CRLF     |
| `wide_chars.bin`  | 13    | CJK width-2 graphemes adjacent to ASCII         |

Phase B expands the catalog to include ligatures, emoji ZWJ,
scrollback, resize sequences, and Kitty graphics streams.
