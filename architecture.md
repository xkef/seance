# Architecture

## Data Flow

```
Shell (zsh/bash)
     │
     │ PTY (pseudo-terminal)
     ▼
┌─────────────┐    raw bytes     ┌──────────────┐
│  Terminal   │ ◄──────────────► │ libghostty-vt│
│  (PTY I/O)  │    VT parsing    │  (C via Zig) │
└──────┬──────┘                  └──────────────┘
       │
       │ RenderState / RowIterator / CellIterator
       ▼
┌──────────────┐   shape + rasterize   ┌─────────────┐
│ CellBuilder  │ ◄───────────────────► │ cosmic-text │
│ (frame gen)  │   CacheKey→AtlasEntry │ + SwashCache│
└──────┬───────┘                       └─────────────┘
       │
       │ CellBg[], CellText[], atlas textures
       ▼
┌──────────────┐   3 render passes   ┌───────────┐
│   GpuState   │ ──────────────────► │   wgpu    │
│  (upload +   │   bg_color          │  surface  │
│   draw)      │   cell_bg           │           │
└──────────────┘   cell_text         └───────────┘
```

## Crate Structure

- **`seance-app`** — entry point. Owns the winit event loop, `Window`,
  `App` struct. Drives PTY polling at ~250 Hz via `about_to_wait`,
  redraws on dirty. Handles keyboard/mouse dispatch.
- **`seance-terminal`** — the core. Contains `Terminal` (VT + PTY),
  `TerminalRenderer` (font + GPU), and all submodules.
- **`seance-input`** — translates winit key/mouse events into VT escape
  sequences using libghostty-vt's key/mouse encoders. Handles Cmd
  shortcuts (quit, copy, paste, font size).

## Terminal (VT + PTY)

`terminal.rs` wraps:

- **libghostty-vt `VtTerminal`** — the VT state machine. Receives raw
  bytes via `vt_write()`, maintains the cell grid, cursor, modes,
  scrollback.
- **portable-pty** — spawns a shell, provides a `MasterPty` with
  non-blocking reader/writer.
- **Selection** — character/word/line selection state with `GridPos`
  ranges.

The `poll()` method reads from the PTY fd (non-blocking, up to 4096
bytes per call), feeds data to the VT emulator, and flushes any VT
response bytes (device status reports etc.) back to the PTY.

## Renderer

`TerminalRenderer` in `renderer.rs` bridges VT state and the GPU:

```
TerminalRenderer
├── GlyphCache        (font system + atlas + glyph map)
├── CellBuilder       (VT → GPU buffer conversion)
├── GpuState          (wgpu device, surface, pipelines)
├── Theme             (colors, palette)
├── Overlay           (cursor shape/pos, selection range)
└── cell_size, grid_padding, surface dimensions
```

### Font Pipeline (`font/`)

**FontSystem** (`system.rs`):

- Wraps `cosmic_text::FontSystem` (discovers system fonts via fontdb)
  and `SwashCache` (glyph rasterizer).
- Computes `CellMetrics` by shaping a single "M" glyph: `cell_width` =
  advance, `cell_height` = line height (font_size × 1.2), `baseline` =
  80% of cell height.
- All metrics are in physical pixels (font_size × scale).

**GlyphAtlas** (`atlas.rs`):

- Two planes: **grayscale** (2048×2048, R8, 1 bpp) for regular text,
  **color** (1024×1024, RGBA8, 4 bpp) for emoji.
- Each plane uses `etagere::AtlasAllocator` for rectangle packing.
- `insert()` allocates a rectangle, copies the bitmap row-by-row into
  the atlas data buffer, marks dirty.

**GlyphCache** (`cache.rs`):

- `HashMap<cosmic_text::CacheKey, AtlasEntry>` with FxHash for speed.
- `get_or_insert(key)`: on miss, calls `SwashCache::get_image()` to
  rasterize the glyph, inserts into atlas, stores the entry (atlas
  position, size, bearings, color flag).
- On font size change: clears the map and resets both atlas planes.

**CellBuilder** (`cell_builder.rs`):

Per-frame work. `build_frame()` does:

1. `RenderState::new()` → `render_state.update(vt)` → snapshot of the
   VT grid.
2. Iterates every row/cell via `RowIterator`/`CellIterator`.
3. **Background**: reads `cell.bg_color()` or resolves `style.bg_color`
   against the theme palette → pushes `[u8; 4]` RGBA to
   `bg_cells[row * cols + col]`.
4. **Text**: reads `cell.graphemes()`, shapes with cosmic-text
   (`Buffer::new` → `set_text` → `shape_until_scroll`), gets `CacheKey`
   for each shaped glyph, calls `glyph_cache.get_or_insert()` → builds
   a `CellText` struct.
5. Computes grid padding (center the grid in the surface).
6. Returns `FrameInfo` (cell size, grid dims, padding, bg color, cursor
   state).

**CellText layout** (32 bytes, matches WGSL vertex buffer):

```
offset  0: glyph_pos      [u32; 2]   atlas pixel coords
offset  8: glyph_size     [u32; 2]   bitmap dimensions
offset 16: bearings       [i16; 2]   x/y bearing for positioning
offset 20: grid_pos       [u16; 2]   terminal column, row
offset 24: color          [u8; 4]    RGBA foreground (Unorm8x4)
offset 28: atlas_and_flags u32       low byte = atlas (0=gray, 1=color)
                                      byte 1 = flags (bit 0 = cursor glyph)
```

### GPU Pipeline (`gpu/`)

**GpuState** (`state.rs`):

- Creates wgpu device with `HighPerformance` adapter, `AutoVsync`
  present mode.
- Surface format: non-sRGB (e.g. `Bgra8Unorm`) for gamma-space alpha
  blending.
- On macOS: sets `CAMetalLayer.presentsWithTransaction = true` to
  prevent stretching during live resize.

**Pipelines** (`pipeline.rs`):

3 bind group layouts: uniforms (group 0), bg_cells storage (group 1),
atlas textures + sampler (group 2).

3 render pipelines, all in one render pass:

| Pass | Pipeline    | Vertex              | Fragment                                       | Blend              |
|------|-------------|---------------------|-------------------------------------------------|--------------------|
| 1    | `bg_color`  | fullscreen triangle | solid `uniforms.bg_color`                       | none (opaque)      |
| 2    | `cell_bg`   | fullscreen triangle | per-cell bg + selection highlight + cursor       | premultiplied alpha|
| 3    | `cell_text` | instanced quads     | atlas sample, min-contrast, cursor color swap    | premultiplied alpha|

**Uniforms** (`uniforms.rs`):

256-byte struct matching the WGSL `Uniforms`. Contains projection
matrix, cell/grid sizes, padding, bg color, min-contrast, cursor state
(VT cursor + overlay cursor), selection range.

**Shaders** (`cell.wgsl`):

- `fs_cell_bg`: reads `bg_cells[row * cols + col]`, blends selection
  color if cell is in selection range, draws cursor shapes
  (block/underline/bar) for the overlay cursor.
- `vs_cell_text`: positions each glyph quad using
  `cell_size * grid_pos + bearings + padding`. Swaps foreground to
  cursor color when at cursor position.
- `fs_cell_text`: samples grayscale atlas (alpha mask × fg color) or
  color atlas (direct RGBA). Applies WCAG min-contrast correction
  against the cell's background.

**Per-frame GPU work** in `render_frame()`:

1. Write uniforms buffer.
2. Upload `bg_cells` to storage buffer (recreate if size changed).
3. Upload `text_cells` to vertex buffer (instanced).
4. Upload atlas textures (grayscale R8, color RGBA8).
5. Execute 3 passes in a single render pass.
6. Present.

## macOS 26 Workaround

Zig 0.15's linker targets `arm64`, but the macOS 26 SDK's
`libSystem.tbd` only declares `arm64e`. `tools/xcrun` redirects Zig's
SDK detection to a sysroot overlay that uses Zig's own bundled
`libSystem.tbd` (which includes `arm64`). Run `tools/setup-sysroot.sh`
once after cloning. This workaround can be removed once Zig ships with
macOS 26 linker stubs.
