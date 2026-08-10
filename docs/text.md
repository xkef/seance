# Text rendering

The decisions that govern how a glyph travels from a font file to a pixel —
which shaper runs, what the atlas stores, whether origins snap to the pixel
grid, whether hinting is on — live across `crates/seance-render/src/text/`. They
are easy to perturb by accident during a refactor and hard to recover once lost,
because most are the _absence_ of a knob (no subpixel quantization, no
per-platform shaper) rather than a line of code that names itself.

This document writes those invariants down and grounds each in the file and
function that enforces it. It is also the spec a future CoreText backend would
inherit, so that backend reproduces these defaults instead of re-deriving them
from scratch. Where seance diverges from Ghostty, or defers a decision to a
dependency, that is called out rather than papered over.

The companion overview is [`docs/architecture.md`](./architecture.md) §Renderer;
this file is the detail behind that section's "Font pipeline" bullets.

## 1. Shaping

Shaping is cosmic-text everywhere, through one backend. `CosmicTextBackend`
(`text/cosmic.rs`) is the only implementation of the `TextBackend` trait
(`text/backend.rs`) and the sole crate-local user of `cosmic_text`; swapping to
parley or a hand-rolled stack stays a file-local change. There is no
per-platform shaping fork planned for v1 — macOS and Linux shape identically.

`shape_run` (`text/cosmic.rs`) shapes one contiguous same-style run of cells at
a time with `Shaping::Advanced`, so ligatures (`==`, `=>`), regional-flag pairs,
and ZWJ sequences compose across cell boundaries. Each emitted `ShapedGlyph`
carries its source-cluster byte offset (`cluster`); the cell builder
(`text/cell_builder.rs`, `RunBuilder::cell`) anchors the glyph at the column the
cluster originated in, which is what lets a multi-cell grapheme keep its grid
alignment.

OpenType features are user-supplied tags parsed once into a cosmic-text
`FontFeatures` list (`build_font_features` in `text/cosmic.rs`) and applied to
every shape call via `Attrs::font_features`. A tag that is not exactly four
bytes is dropped with a warning rather than silently disabling shaping for the
cell.

## 2. Rasterization

Outlines are rasterized by Swash through cosmic-text's `SwashCache`
(`CosmicTextBackend::rasterize` in `text/cosmic.rs`), which covers COLR v0/v1,
SVG, and CBDT. A rasterized glyph reports its coverage bitmap, its pixel
dimensions, integer left/top bearings, and a `GlyphFormat` of either `Alpha`
(grayscale coverage) or `Color` (full BGRA), keyed off Swash's `SwashContent`.

The `GlyphAtlas` (`text/atlas.rs`) keeps two planes, never one shared surface:

- **grayscale** — R8, 2048×2048, one byte per pixel (`GRAYSCALE_SIZE`, `bpp` 1).
- **color** — RGBA8, 1024×1024, four bytes per pixel (`COLOR_SIZE`, `bpp` 4).

`GlyphAtlas::insert` routes by the glyph's `is_color` flag, packs the rectangle
with `etagere` (shelf-bin packing that can deallocate, unlike a row-packer), and
marks the owning plane dirty so only changed planes re-upload. An empty glyph
(zero width or height) is dropped before it reaches the atlas
(`CosmicTextBackend::rasterize`), so whitespace never consumes a slot.

## 3. Subpixel positioning

Glyph origins snap to the integer pixel grid; bitmaps are **not** quantized into
subpixel variants. The load-bearing line is the cache key built in `shape_run`
(`text/cosmic.rs`): the `CacheKey` is constructed with a subpixel offset of
`(0.0, 0.0)`, fixed for every glyph. That single choice means each glyph
rasterizes once and occupies exactly one atlas entry, regardless of its
fractional pen position within a run.

This is deliberate and matches Ghostty's "subpixel positioning enabled, subpixel
quantization disabled" stance: terminals are cell-aligned, so the extra atlas
entries of a Zed-style four-variant quantization buy nothing here. The glyph's
placement within its cell comes from the integer `bearing_x` / `bearing_y`
stored on the `AtlasEntry` and emitted into the `CellText` instance
(`text/cell_builder.rs`), not from a fractional origin.

## 4. Hinting

Seance exposes no hinting knob; hinting is whatever cosmic-text and Swash apply
by default for the rasterized face. There is no per-glyph constraint transform
in the pipeline today, so the Ghostty rule below has nothing to switch off yet —
it is recorded here as the target semantics for the day a constraint transform
(or a CoreText backend) lands.

The Ghostty rule to mirror: hinting is on for cell-aligned glyphs and is
auto-disabled whenever a glyph constraint transforms the outline, because the
transform invalidates the hints (`font/face/freetype.zig`,
`do_hinting = load_flags.hinting and !constrained`). Any future constraint path
in seance should adopt the same "transform ⇒ no hinting" coupling rather than
inventing a new policy.

## 5. macOS CoreText backend (future)

No CoreText backend exists; `CosmicTextBackend` is the only `TextBackend`. If
one is ever added, it inherits the CG bitmap-context defaults Ghostty uses
(`font/face/coretext.zig`), so the two backends agree pixel-for-pixel on the
shared cases:

- `setAllowsFontSubpixelPositioning(true)` and
  `setShouldSubpixelPositionFonts(true)` — subpixel positioning on.
- `setAllowsFontSubpixelQuantization(false)` and
  `setShouldSubpixelQuantizeFonts(false)` — quantization off, matching §3's
  one-entry-per-glyph invariant.
- `setShouldAntialias(true)` — antialiasing on.
- `setShouldSmoothFonts(thicken)` — font smoothing tracks the faux-bold
  ("thicken") knob. This smoothing is what macOS users perceive as the "font
  weight" effect, so it must be driven by that toggle and nothing else.

## 6. Color glyphs and emoji

Color glyphs are routed end to end through the color path. `rasterize`
(`text/cosmic.rs`) tags a Swash `Color` bitmap as `GlyphFormat::Color`; the cell
builder forwards `is_color` to `GlyphAtlas::insert` (`text/cell_builder.rs`,
`ensure_glyph_slot`), which lands it in the BGRA plane (§2). Any future
coverage-domain transform (a faux-bold dilation, say) must stay grayscale-only
and leave these bitmaps alone, since dilating premultiplied RGBA would smear
color rather than darken it.

Presentation selectors (U+FE0E text-style, U+FE0F emoji-style) are **not**
handled explicitly in seance: whatever disambiguation happens is whatever
cosmic-text / rustybuzz perform during shaping. An explicit
presentation-selector policy at shape-run iteration is not yet implemented;
until it is, do not assume a given emoji base resolves to text or color
presentation based on a trailing selector.
