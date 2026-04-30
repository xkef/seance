# Architecture

This document describes both **what is built** and **what is planned**. Each
section is tagged `[IMPLEMENTED]` (present in `main`/feature branches) or
`[PLANNED: M<n>]` (scheduled under a GitHub epic, linked inline).

Epic index:

- **[M1][m1]** — Config & theme foundations
- **[M2][m2]** — Rendering performance (shape cache, dirty rows, sync output,
  deadline redraw, IO thread)
- **[M3][m3]** — Visual fidelity (procedural glyphs, WCAG contrast, clipboard)
- **[M4][m4]** — Z-layer architecture refactor
- **[M5][m5]** — Image protocols (Kitty graphics residuals, animation, iTerm2)
- **[M6][m6]** — Multiplexing (`seance-mux` crate, tabs, splits, floating
  modals)
- **[M7][m7]** — Custom shaders (Shadertoy-compatible post-pass)
- **[M8][m8]** — Lua scripting + widget system
- **[M9][m9]** — Release pipeline & distribution (Homebrew, AUR, apt)
- **[M10][m10]** — Agent Plane (in-PTY control, UI ownership, coordination)
- **[M11][m11]** — Test harness (layered, LLM-readable)

[m1]: https://github.com/xkef/seance/issues/4
[m2]: https://github.com/xkef/seance/issues/5
[m3]: https://github.com/xkef/seance/issues/6
[m4]: https://github.com/xkef/seance/issues/7
[m5]: https://github.com/xkef/seance/issues/8
[m6]: https://github.com/xkef/seance/issues/9
[m7]: https://github.com/xkef/seance/issues/10
[m8]: https://github.com/xkef/seance/issues/65
[m9]: https://github.com/xkef/seance/issues/152
[m10]: https://github.com/xkef/seance/issues/194
[m11]: https://github.com/xkef/seance/issues/201

---

## Pipeline overview

```
┌─ input ──────────────────────────────────────────────────────────────┐
│ winit event loop → seance-input                                      │
│   key: KeyboardEvent → libghostty-vt key encoder → utf-8 bytes       │
│   mouse: wheel/click → SGR 1006 encoding → bytes                     │
│                           │                                          │
│                           ▼                                          │
│                       master PTY fd ──── write ────▶ shell           │
└──────────────────────────────────────────────────────────────────────┘

┌─ PTY read pump ──────────────────────────────────────────────────────┐
│ seance-pty-reader thread: MasterPty.read() → UserEvent::PtyData      │
│ UI thread: libghostty-vt.write() → VT state machine mutates grid     │
│ row-dirty bitmap [IMPLEMENTED]                                       │
│ DEC 2026 synchronized output [PLANNED: M2]                           │
│ IO-thread parse + Critical-snapshot [PLANNED: M2 — see threading.md] │
└──────────────────────────────────────────────────────────────────────┘

┌─ render pass (wakes on dirty + animation deadline) ──────────────────┐
│ rebuild_cells(only_dirty_rows [PARTIAL: bg_cells]):                  │
│   for each row: run-iterator → TextRuns                              │
│     shape_cache.get_or_shape(run_hash)  [PLANNED: M2]                │
│       cosmic-text Buffer::shape_until_scroll                         │
│     for each glyph:                                                  │
│       procedural sprite registry [PLANNED: M3]                       │
│       glyph_cache.get_or_insert(CacheKey)                            │
│         miss → SwashCache → bitmap → etagere atlas                    │
│                                                                      │
│ emit quads into per-layer vertex buffers [PLANNED: M4]               │
│ single render pass → N pipeline switches                             │
│ optional post-pass (custom shaders, ping-pong) [PLANNED: M7]         │
│ present()                                                            │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Crate structure

| Crate           | Owns                                               | Status              |
| --------------- | -------------------------------------------------- | ------------------- |
| `seance-app`    | winit event loop, `App`, render-thread driver      | [IMPLEMENTED]       |
| `seance-input`  | winit → VT key/mouse encoding (via libghostty-vt)  | [IMPLEMENTED]       |
| `seance-render` | font pipeline, GPU pipelines, GlyphAtlas           | [IMPLEMENTED]       |
| `seance-vt`     | libghostty-vt wrapper, portable-pty PTY, selection | [IMPLEMENTED]       |
| `seance-mux`    | Domain → Window → Tab → SplitTree → Pane           | [PLANNED: [M6][m6]] |

---

## VT layer (`seance-vt`)

- **libghostty-vt** [IMPLEMENTED] — VT state machine via FFI. Handles
  CSI/OSC/DCS, alt screen, scrollback, mouse modes, Kitty keyboard.
- **portable-pty** [IMPLEMENTED] — cross-platform PTY (ConPTY on Windows).
- **FrameSource** trait [IMPLEMENTED] — exposes `visit_cells()` to the renderer.
- **Row-dirty flags** [IMPLEMENTED] — `dirty_rows()` iterator over the VT grid
  (#191). The renderer uses it for partial `bg_cells` upload (#196); text-cell
  rebuild still walks the full grid pending shape cache (#21).
- **DEC 2026 synchronized output** [PLANNED: [M2][m2]] — `is_sync_active()` +
  timeout, suppress rebuild while mode is set.
- **OSC 52 clipboard** [PLANNED: [M3][m3]] — read/write with paste-protection
  prompt.
- **Kitty graphics protocol** [PLANNED: [M5][m5]] — transmission, placements,
  virtual placeholders (U+10EEEE).

---

## Renderer (`seance-render`)

### Font pipeline

- **cosmic-text** [IMPLEMENTED] — wraps fontdb + rustybuzz + bidi. Shapes
  contiguous same-style runs of cells through `TextBackend::shape_run`, so
  ligatures (`==`, `=>`), regional flag pairs, and ZWJ sequences compose across
  cell boundaries; each emitted glyph carries its source-cluster byte offset so
  the cell builder anchors it at the originating column.
- **OpenType features** [IMPLEMENTED] — `font.features` is parsed into a
  cosmic-text `FontFeatures` list and applied via `Attrs::font_features`.
- **SwashCache** [IMPLEMENTED] — rasterizes outlines (COLR v0/v1, SVG, CBDT).
- **GlyphAtlas** [IMPLEMENTED] — two planes: grayscale R8 (2048×2048) and color
  RGBA8 (1024×1024). `etagere` rectangle packing, per-plane `dirty` flag.
- **GlyphCache** [IMPLEMENTED] — `FxHashMap<cosmic_text::CacheKey, AtlasEntry>`.
- **Shape cache** keyed by `(font flags, run bytes)` [IMPLEMENTED] — 256-bucket
  × 8-way LRU; the key omits color so per-frame palette flicker doesn't evict.
- **Procedural sprite registry** (codepoints > U+10FFFF, U+2500–U+259F,
  U+E0B0–U+E0B3, legacy computing, braille) [PLANNED: [M3][m3]] — rasterized via
  `tiny-skia`, intercepted before cosmic-text shaping.

### CellBuilder

- **Current** [IMPLEMENTED] — iterates entire VT grid each frame, groups
  contiguous same-style cells into shape runs, dispatches each run through
  `TextBackend::shape_run`, then anchors each emitted glyph at its
  source-cluster column before writing `text_cells` SSBO data; `bg_cells` upload
  is dirty-row-batched (#196).
- **Target** — takes `&[PositionedPane]` [PLANNED: [M6][m6]], only iterates
  dirty rows for text rebuild [PLANNED: [M2][m2]].

### CellText instance layout (matches WGSL, 32 bytes)

| Offset | Field             | Type    | Purpose                                                        |
| ------ | ----------------- | ------- | -------------------------------------------------------------- |
| 0      | `glyph_pos`       | `u32×2` | atlas pixel coords                                             |
| 8      | `glyph_size`      | `u32×2` | bitmap dimensions                                              |
| 16     | `bearings`        | `i16×2` | x/y bearing                                                    |
| 20     | `grid_pos`        | `u16×2` | column, row                                                    |
| 24     | `color`           | `u8×4`  | RGBA foreground (Unorm8x4)                                     |
| 28     | `atlas_and_flags` | `u32`   | low byte: atlas (0=gray,1=color); byte 1: flags (bit 0=cursor) |

---

## GPU layers

### Current pipeline [IMPLEMENTED]

One `wgpu::RenderPass` with 3 pipelines sharing 3 bind groups (uniforms /
bg_cells SSBO / atlas textures + sampler):

| Pass | Pipeline    | Vertex              | Fragment                                          | Blend               |
| ---- | ----------- | ------------------- | ------------------------------------------------- | ------------------- |
| 1    | `bg_color`  | fullscreen triangle | solid uniforms.bg_color                           | none                |
| 2    | `cell_bg`   | fullscreen triangle | per-cell bg from SSBO + selection + cursor shapes | premultiplied alpha |
| 3    | `cell_text` | instanced quads     | atlas sample, min-contrast, cursor color swap     | premultiplied alpha |

### Target layer stack [PLANNED: [M4][m4]]

`RenderLayer` enum backed by per-layer triple vertex buffers, sorted CPU-side,
no depth buffer:

```
Layer 0  BgImage
Layer 1  BgFill
Layer 2  KittyUnder        ← Kitty graphics z < min
Layer 3  CellBg             ← fullscreen tri + cells_bg SSBO
Layer 4  KittyMid           ← Kitty graphics 0 > z >= min
Layer 5  CellText           ← glyphs + sprite underlines + cursor glyph
Layer 6  KittyOver          ← Kitty graphics z >= 0
Layer 7  CursorOver         ← cursor-over-text sprite
Layer 8  Selection          ← selection rect overlay
Layer 9  StatusBar/TabBar   ← [PLANNED: M4/M6]
Layer 10 Modal              ← command palette, char select [PLANNED: M6]
Layer 11 ImePreedit         ← inline IME composition [PLANNED: M6]
```

### Offscreen post-pass infrastructure [PLANNED: [M4][m4] + [M7][m7]]

Front/back `bgra8unorm_srgb` render textures sized to the surface. All layers
target `back`; optional ping-pong of user-supplied Shadertoy-compatible shaders;
final blit to the drawable.

### Atlas upload

`wgpu::Queue::write_texture` per inserted glyph. Migrate to dirty-sub-rect
batching [PLANNED: [M2][m2]].

---

## Event loop & redraw

### Current [IMPLEMENTED]

Deadline-scheduled (`cf4a1b1`, #24): `ControlFlow::WaitUntil(next_due)` across
all animation sources — cursor blink, SGR blink, bell, Kitty GIF frames,
custom-shader animation. Idle terminal = 0 fps. Modelled on WezTerm's
`has_animation` pattern. PTY wakes are out-of-band via `EventLoopProxy`, fed by
the `seance-pty-reader` thread (`crates/seance-app/src/io.rs`).

### Threading model

VT parsing still runs on the winit thread inside `App::user_event(PtyData)`.
[M2][m2] moves it to a dedicated IO thread that owns VT + PTY behind
`Arc<parking_lot::FairMutex<VtState>>`; the UI takes a brief locked snapshot
each frame and rebuilds cells outside the lock (Ghostty's `Critical` pattern).
Full design, mailbox protocol, lock budget, DEC 2026 watchdog, shutdown
ordering, and the renderer-thread revisit metric: see
[`docs/threading.md`](./threading.md).

---

## Multiplexing model [PLANNED: [M6][m6]]

New `seance-mux` crate:

```
Domain (trait)                       ← LocalDomain wraps portable-pty
  └─ Window
       └─ Tab
            └─ SplitTree = Leaf(Pane) | Split(dir, ratio, left, right)
```

`fn panes_positioned(&self, pixel_rect: Rect) -> Vec<PositionedPane>` walks the
tree and emits per-pane `{ cell_rect, pixel_rect, is_active, pane }`.
`CellBuilder` offsets `grid_pos` by each pane's top-left. **All panes render
into one framebuffer** — no render-target-per-pane.

- Split borders: 1px quads via floating-quad emitter (`RenderLayer::Selection`).
- Inactive-pane dimming: shader uniform `inactive_pane_hsb: vec3<f32>`, applied
  to fg when `pane_idx != active_pane_idx`.
- Tab bar: reserved row rendered through the status-line path.
- Floating modals (palette, char select): `taffy` box-model →
  `RenderLayer::Modal`.
- IME preedit: winit `Ime::Preedit` → shape inline at cursor column,
  `RenderLayer::ImePreedit`.

---

## Config surface

### Current [IMPLEMENTED]

Compile-time defaults only. `RendererConfig` exposes `width`, `height`, `scale`,
`font_family` (hardcoded "JetBrainsMono Nerd Font"), `font_size`
(runtime-adjustable via keybinds). Theme is `impl Default` → Catppuccin Frappe
palette.

### Target [PLANNED: [M1][m1]]

```toml
# ~/.config/seance/config.toml
[font]
family = "JetBrainsMono Nerd Font"
size = 14.0
features = ["calt"]
min_contrast = 1.1
adjust_cell_height = 1.20

[window]
padding_x = 12
padding_y = 0
decoration = true
background_opacity = 1.0

[cursor]
style = "bar"            # block | bar | underline
blink = false

[clipboard]
read = "ask"             # ask | allow | deny
write = "allow"
paste_protection = true
copy_on_select = true

[scrollback]
limit = 50000

[mouse]
hide_while_typing = true

[input]
macos_option_as_alt = "none"  # or "left" / "right" / "both"

[[keybind]]
key = "ctrl+shift+c"; action = "copy"

[renderer]
custom_shaders = []
custom_shader_animation = "focused"

theme = "Catppuccin Frappe"  # resolves ~/.config/seance/themes/Catppuccin Frappe.toml
```

Theme files ship Catppuccin / Gruvbox / Tokyo Night / Solarized. Hot-reload via
`notify` with targeted invalidation (theme → repaint; font → clear glyph + shape
caches; keybind → rebuild action table).

---

## Input

- winit `KeyboardInput` → `seance-input` → libghostty-vt key encoder
  [IMPLEMENTED].
- `input.macos_option_as_alt` = `none` / `left` / `right` / `both`
  [IMPLEMENTED]. When `left` or `right`, only that side of Option sends
  `ESC`-prefix; the other side falls through to macOS text composition (`ø`,
  `¬`, `–`, …). `both` makes both Option keys Alt (breaks text composition);
  `none` (default) preserves the macOS default.
- User keybind table parsed from config → `Action` enum (`Copy`, `Paste`,
  `FontSize(i8)`, `NewTab`, `SplitH`, `FocusPane(Dir)`, `ToggleFullscreen`, …)
  [PLANNED: [M1][m1] + [M6][m6]].

---

## Platform notes

- macOS IOSurface / `CAMetalLayer.presentsWithTransaction = true` to prevent
  live-resize stretching [IMPLEMENTED].
- macOS 26 SDK + Zig 0.15 linker workaround (`tools/xcrun` redirects SDK sysroot
  to Zig's bundled `libSystem.tbd`) [IMPLEMENTED].
- Wayland damage tracking via `swap_buffers_with_damage` is not required for
  wgpu — dirty-row uploads + deadline-driven redraw replace it.

---

## Appendix — component choices

| Problem                   | Component                                               | Why                                                                                                                                         |
| ------------------------- | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| GPU API                   | `wgpu`                                                  | One abstraction for Metal/Vulkan/DX12/GL4/WebGPU. Dual-source blending (for LCD subpixel AA) gated behind `Features::DUAL_SOURCE_BLENDING`. |
| Window + input            | `winit`                                                 | Only serious cross-platform option.                                                                                                         |
| VT state machine          | `libghostty-vt` via FFI                                 | Battle-tested, handles DEC 2026, mouse, Kitty keyboard, iTerm OSC, selection. Don't reinvent.                                               |
| PTY                       | `portable-pty`                                          | Cross-plat, correct ConPTY on Windows.                                                                                                      |
| Font discovery            | `fontdb` (via cosmic-text)                              | fontconfig / CoreText / DirectWrite backed.                                                                                                 |
| Shaping                   | `cosmic-text` (rustybuzz + unicode-bidi)                | BiDi, graphemes, per-font features.                                                                                                         |
| Rasterization             | `swash` (via `SwashCache`)                              | COLR v0/v1, SVG, CBDT.                                                                                                                      |
| Atlas packing             | `etagere`                                               | Shelf-bin with deallocation (alacritty's row-packer cannot evict).                                                                          |
| Procedural glyphs         | `tiny-skia`                                             | Software vector rasterizer for box-drawing / Powerline sprites [PLANNED: [M3][m3]].                                                         |
| Layout (modals/box model) | `taffy`                                                 | Flexbox + Grid for floating UI [PLANNED: [M6][m6]].                                                                                         |
| Animation                 | In-house `ColorEase` + deadline scheduler [IMPLEMENTED] | Cubic-bezier ease, `ControlFlow::WaitUntil(min(next_due))` — power-efficient.                                                               |
| Config                    | `toml` + `serde` + `notify`                             | Hot-reload with targeted invalidation.                                                                                                      |
| Logging                   | `tracing` + `tracing-subscriber`                        | Per-subsystem spans.                                                                                                                        |

**Deliberately avoided:** `fontdue` (no COLRv1/SVG), `glyphon` (locks layout),
`vello`/`wgpu_glyph` (wrong abstraction level for terminals), hand-rolled VT
parsers (tarpit — every terminal team regrets them).

---

## Reference terminals

For threading-model rationale (Ghostty / Alacritty / WezTerm side-by-side), see
[`docs/threading.md`](./threading.md). Source citations into each upstream tree
live there, not here.
