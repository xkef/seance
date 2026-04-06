Plan: Switch to libghostty-rs for VT, split FontGrid from Renderer

Context

Seance uses a monolithic lib_renderer_c.zig that bundles terminal emulation
(ghostty_terminal_*) and rendering (ghostty_renderer_*) in one C API. The
Rust side mirrors this with ghostty-renderer-sys + ghostty-renderer
containing both Renderer and Terminal.

An upstream crate (github.com/Uzaaft/libghostty-rs) provides idiomatic Rust
bindings for libghostty-vt with effects callbacks, dirty tracking, render state
iteration, and key/mouse encoding. Switching to it lets us:

1. Drop all ghostty_terminal_* from our fork (use libghostty-vt instead)
2. Drop Level 1 (draw-to-surface) API we never use
3. Split renderer into FontGrid + Renderer (shared atlas for multiplexer)

Decisions: Git dependency + GHOSTTY_SOURCE_DIR for libghostty-rs.
FontGrid/Renderer split now.

---
Phase 1: Add libghostty-rs, rewrite Terminal

Files:
- Cargo.toml — add workspace deps for libghostty-vt, libghostty-vt-sys
- crates/seance-terminal/Cargo.toml — depend on libghostty-vt
- crates/seance-terminal/src/terminal.rs — rewrite VT wrapper

The Terminal struct wraps libghostty_vt::Terminal instead of
ghostty_renderer::Terminal. PTY responses use on_pty_write callback with
Rc<RefCell<Vec<u8>>> instead of drain/clear pattern.

Key API mapping:

┌─────────────────────────┬─────────────────────────────────────────────┐
│           Old           │                     New                     │
├─────────────────────────┼─────────────────────────────────────────────┤
│ gr::Terminal::new(c, r) │ vt::Terminal::new(TerminalOptions{...})?    │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.vt_write(data)       │ terminal.vt_write(data)                     │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.resize(c, r)         │ terminal.resize(c, r, cw, ch)?              │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.drain_responses()    │ on_pty_write callback collects to buffer    │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.size()               │ (terminal.cols()?, terminal.rows()?)        │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.cursor()             │ cursor_x(), cursor_y(), is_cursor_visible() │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.scroll(action)       │ terminal.scroll_viewport(scroll)            │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.mode_cursor_keys()   │ terminal.mode(Mode::CursorKeys)?            │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.mode_mouse_event()   │ terminal.is_mouse_tracking()?               │
├─────────────────────────┼─────────────────────────────────────────────┤
│ vt.dump_screen()        │ RenderState row/cell iteration (later)      │
└─────────────────────────┴─────────────────────────────────────────────┘

Phase 2: Split lib_renderer_c.zig — FontGrid + Renderer, remove terminal

File: ghostty/src/lib_renderer_c.zig

Delete: TerminalWrapper, all ghostty_terminal_* exports,
ghostty_renderer_draw_frame, ghostty_free.

Split RendererState into:

const FontGridState = struct {
    alloc: Allocator,
    font_grid_set: font.SharedGridSet,
    font_grid: *font.SharedGrid,
    font_grid_key: font.SharedGridSet.Key,
    config: configpkg.Config,
    atlas_grayscale_gen: usize = 0,
    atlas_color_gen: usize = 0,
};

const RendererState = struct {
    alloc: Allocator,
    renderer: Renderer,
    render_state: rendererpkg.State,
    font_grid_state: *FontGridState,  // borrowed
    terminal_set: bool = false,
    mutex: std.Thread.Mutex = .{},
    text_cells_staging: []CellText = &.{},
};

New exports:

// Font grid
ghostty_font_grid_new(config) -> ?*FontGridState
ghostty_font_grid_free(grid)
ghostty_font_grid_get_metrics(grid, out)
ghostty_font_grid_atlas_grayscale(grid, size, modified) -> [*]const u8
ghostty_font_grid_atlas_color(grid, size, modified) -> [*]const u8
ghostty_font_grid_set_size(grid, points)

// Renderer (takes font grid + renderer config)
ghostty_renderer_new(grid, config) -> ?*RendererState
ghostty_renderer_free(r)
ghostty_renderer_set_terminal(r, raw_ptr)  // raw *terminal.Terminal
ghostty_renderer_resize(r, w, h)
ghostty_renderer_update_frame(r, cursor_blink_visible)
ghostty_renderer_bg_cells(r, count) -> [*]const [4]u8
ghostty_renderer_text_cells(r, count) -> ?[*]const CellText
ghostty_renderer_frame_data(r, out)

// Runtime config (on renderer)
ghostty_renderer_set_background/foreground/opacity/contrast/palette
ghostty_renderer_load_theme/load_theme_file

set_terminal accepts *anyopaque — the raw TerminalImpl pointer from
libghostty-vt. Casts to *terminal.Terminal (same Zig struct, same source).

Phase 3: Update renderer.h

File: ghostty/include/ghostty/renderer.h

Match the new C API. Remove all terminal types/functions. Add FontGrid types.
Two opaque handles: GhosttyFontGrid, GhosttyRenderer.

Phase 4: Update Rust FFI + safe wrapper

Files:
- crates/ghostty-renderer-sys/src/lib.rs — new FFI declarations
- crates/ghostty-renderer/src/lib.rs — FontGrid + Renderer, no Terminal
- crates/ghostty-renderer/Cargo.toml — add libghostty-vt-sys dep

pub struct FontGrid { raw: NonNull<c_void>, ... }
pub struct Renderer { raw: NonNull<c_void>, _grid: Arc<FontGrid>, ... }

impl FontGrid {
    fn new(config: &FontGridConfig) -> Option<Self>;
    fn metrics(&self) -> FontMetrics;
    fn atlas_grayscale(&self) -> AtlasTexture;
    fn atlas_color(&self) -> AtlasTexture;
    fn set_size(&self, points: f32);
}

impl Renderer {
    fn new(grid: Arc<FontGrid>, config: &RendererConfig) -> Option<Self>;
    fn set_terminal(&self, t: &libghostty_vt::Terminal);
    fn resize(&self, w: u32, h: u32);
    fn update_frame(&self, cursor_blink: bool);
    fn frame_snapshot(&self) -> FrameSnapshot;
    // + runtime config setters
}

Phase 5: Update seance-terminal consumers

Files:
- crates/seance-terminal/src/terminal.rs — use libghostty_vt::Terminal
- crates/seance-terminal/src/renderer.rs — shared Arc, new API
- crates/seance-terminal/src/lib.rs — update re-exports
- crates/seance-app/src/main.rs — adapt initialization

The TerminalRenderer gains font_grid: Arc<FontGrid> shared across panes.

What gets deleted

- TerminalWrapper + all ghostty_terminal_* in lib_renderer_c.zig (~200 LOC)
- ghostty_renderer_draw_frame (Level 1)
- Terminal type from ghostty-renderer crate
- ScrollAction, CursorState, mode queries from ghostty-renderer
- Terminal FFI declarations from ghostty-renderer-sys
- Terminal/scroll types from renderer.h

Build integration

- libghostty-rs added as git dependency (github.com/Uzaaft/libghostty-rs)
- GHOSTTY_SOURCE_DIR env var points to ghostty/ submodule
- Both libghostty-vt (.dylib) and libghostty-renderer (.a) build from same source
- ABI compatibility guaranteed by shared source

Verification

1. cd ghostty && zig build — produces libghostty-renderer.a
2. GHOSTTY_SOURCE_DIR=ghostty cargo build — all crates compile
3. cargo run -p seance-app — terminal renders, text works
4. Verify: scrolling, resize, cursor, mouse wheel, theme

