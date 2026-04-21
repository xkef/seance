# Architecture

This document describes both **what is built** and **what is planned**. Each
section is tagged `[IMPLEMENTED]` (present in `main`/feature branches) or
`[PLANNED: M<n>]` (scheduled under a GitHub epic, linked inline).

Epic index:

- **[M1][m1]** — Config & theme foundations
- **[M2][m2]** — Rendering performance (shape cache, dirty rows, sync output,
  deadline redraw)
- **[M3][m3]** — Visual fidelity (procedural glyphs, WCAG contrast, clipboard)
- **[M4][m4]** — Z-layer architecture refactor
- **[M5][m5]** — Image protocols (Kitty graphics, animated frames)
- **[M6][m6]** — Multiplexing (`seance-mux` crate, tabs, splits, floating
  modals)
- **[M7][m7]** — Custom shaders (Shadertoy-compatible post-pass)

[m1]: https://github.com/xkef/seance/issues/4
[m2]: https://github.com/xkef/seance/issues/5
[m3]: https://github.com/xkef/seance/issues/6
[m4]: https://github.com/xkef/seance/issues/7
[m5]: https://github.com/xkef/seance/issues/8
[m6]: https://github.com/xkef/seance/issues/9
[m7]: https://github.com/xkef/seance/issues/10

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
│ MasterPty.read() → raw bytes                                         │
│ libghostty-vt.write() → VT state machine mutates grid                │
│ DEC 2026 synchronized output [PLANNED: M2]                           │
│ row-dirty bitmap [PLANNED: M2]                                       │
└──────────────────────────────────────────────────────────────────────┘

┌─ render pass (wakes on dirty + animation deadline [PLANNED: M2]) ────┐
│ rebuild_cells(only_dirty_rows [PLANNED: M2]):                        │
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
- **Row-dirty flags** [PLANNED: [M2][m2]] — add `dirty_rows()` iterator so the
  renderer can skip unchanged rows.
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
  grapheme runs per-cell.
- **SwashCache** [IMPLEMENTED] — rasterizes outlines (COLR v0/v1, SVG, CBDT).
- **GlyphAtlas** [IMPLEMENTED] — two planes: grayscale R8 (2048×2048) and color
  RGBA8 (1024×1024). `etagere` rectangle packing, per-plane `dirty` flag.
- **GlyphCache** [IMPLEMENTED] — `FxHashMap<cosmic_text::CacheKey, AtlasEntry>`.
- **Shape cache** keyed by `(font, style, codepoint_run_hash)` [PLANNED:
  [M2][m2]] — bucketed LRU, avoids re-shaping unchanged rows across frames.
- **Procedural sprite registry** (codepoints > U+10FFFF, U+2500–U+259F,
  U+E0B0–U+E0B3, legacy computing, braille) [PLANNED: [M3][m3]] — rasterized via
  `tiny-skia`, intercepted before cosmic-text shaping.

### CellBuilder

- **Current** [IMPLEMENTED] — iterates entire VT grid each frame, shapes every
  visible cell, writes `text_cells` + `bg_cells` vertex/SSBO data.
- **Target** — takes `&[PositionedPane]` [PLANNED: [M6][m6]], only iterates
  dirty rows [PLANNED: [M2][m2]], skips shape when cached [PLANNED: [M2][m2]].

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

`ControlFlow::wait_duration(POLL_INTERVAL = 4ms)` — 250 Hz PTY poll, redraw only
when `content_dirty`, `AutoVsync` surface present mode.

### Target [PLANNED: [M2][m2]]

Deadline-scheduled: one `Timer` at `min(next_due)` across all animation sources
— cursor blink, SGR blink, bell, Kitty GIF frames, DEC 2026 sync timeout,
custom-shader animation. Idle terminal = 0 fps. Modelled on WezTerm's
`has_animation: RefCell<Option<Instant>>` pattern.

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
style = "block"          # block | bar | underline
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
  `ESC`-prefix; the other side falls through to macOS text composition
  (`ø`, `¬`, `–`, …). `both` makes both Option keys Alt (breaks text
  composition); `none` (default) preserves the macOS default.
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

| Problem                   | Component                                                     | Why                                                                                                                                         |
| ------------------------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------- |
| GPU API                   | `wgpu`                                                        | One abstraction for Metal/Vulkan/DX12/GL4/WebGPU. Dual-source blending (for LCD subpixel AA) gated behind `Features::DUAL_SOURCE_BLENDING`. |
| Window + input            | `winit`                                                       | Only serious cross-platform option.                                                                                                         |
| VT state machine          | `libghostty-vt` via FFI                                       | Battle-tested, handles DEC 2026, mouse, Kitty keyboard, iTerm OSC, selection. Don't reinvent.                                               |
| PTY                       | `portable-pty`                                                | Cross-plat, correct ConPTY on Windows.                                                                                                      |
| Font discovery            | `fontdb` (via cosmic-text)                                    | fontconfig / CoreText / DirectWrite backed.                                                                                                 |
| Shaping                   | `cosmic-text` (rustybuzz + unicode-bidi)                      | BiDi, graphemes, per-font features.                                                                                                         |
| Rasterization             | `swash` (via `SwashCache`)                                    | COLR v0/v1, SVG, CBDT.                                                                                                                      |
| Atlas packing             | `etagere`                                                     | Shelf-bin with deallocation (alacritty's row-packer cannot evict).                                                                          |
| Procedural glyphs         | `tiny-skia`                                                   | Software vector rasterizer for box-drawing / Powerline sprites [PLANNED: [M3][m3]].                                                         |
| Layout (modals/box model) | `taffy`                                                       | Flexbox + Grid for floating UI [PLANNED: [M6][m6]].                                                                                         |
| Animation                 | In-house `ColorEase` + deadline scheduler [PLANNED: [M2][m2]] | Cubic-bezier ease, one `Timer` at `min(next_due)` — power-efficient.                                                                        |
| Config                    | `toml` + `serde` + `notify`                                   | Hot-reload with targeted invalidation.                                                                                                      |
| Logging                   | `tracing` + `tracing-subscriber`                              | Per-subsystem spans.                                                                                                                        |

**Deliberately avoided:** `fontdue` (no COLRv1/SVG), `glyphon` (locks layout),
`vello`/`wgpu_glyph` (wrong abstraction level for terminals), hand-rolled VT
parsers (tarpit — every terminal team regrets them).

---

## Reference terminals

For design rationale on any section, see the corresponding reports in `docs/`:

- `docs/LIBGHOSTTY_ANALYSIS.md`, `docs/libghostty_renderer_patterns.md`,
  `docs/libghostty_vt_architecture.md` — Ghostty renderer
- `docs/NOTES.md`, `docs/NOTES2.md`, `docs/NOTES3.md` — cross-terminal research
  (Alacritty, WezTerm, Ghostty)

Side-by-side summary of the three references → see the synthesis in the design
discussion leading up to this plan.
