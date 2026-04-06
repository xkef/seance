Plan: libghostty-renderer C API (Approach B)

Context

Seance is a GPU-rendered terminal multiplexer built on Ghostty's internals. The current fork
adds a libghostty-renderer C API (3 commits, ~1100 lines) that bundles its own terminal API
and couples Level 1 (draw-to-surface) with Level 2 (cell buffers). This plan redesigns the
API to be independent from libghostty-vt, drop Level 1, expose font management separately,
and follow upstream patterns so it can eventually be upstreamed.

The goal: a C library that takes a GhosttyTerminal from libghostty-vt, runs Ghostty's font
shaping + cell buffer generation pipeline, and hands back GPU-ready data (cell arrays + glyph
atlas textures) for the consumer to render with their own pipeline (wgpu in seance's case).

---
API Design

Two opaque handles

GhosttyFontGrid  — shared font system (atlas, shaper, metrics)
GhosttyRenderer  — per-terminal cell buffer generator (bound to one font grid + one terminal)

Separating them lets a multiplexer share one atlas across all panes.

Font Grid API

// Lifecycle
GhosttyRendererResult ghostty_font_grid_new(const GhosttyFontGridConfig *config,
                                             GhosttyFontGrid *out);
void ghostty_font_grid_free(GhosttyFontGrid grid);

// Metrics (cell width/height, baseline, underline/strikethrough positions)
GhosttyRendererResult ghostty_font_grid_get_metrics(GhosttyFontGrid grid,
                                                     GhosttyFontMetrics *out);

// Atlas textures (pointer valid until next update_frame or free)
const uint8_t *ghostty_font_grid_atlas_grayscale(GhosttyFontGrid grid,
                                                  uint32_t *size, bool *modified);
const uint8_t *ghostty_font_grid_atlas_color(GhosttyFontGrid grid,
                                              uint32_t *size, bool *modified);

// Runtime font size change (invalidates atlas, consumer must re-upload)
GhosttyRendererResult ghostty_font_grid_set_size(GhosttyFontGrid grid, float points);

Config struct:
typedef struct {
    float       font_size;              // 0 -> 13.0
    const char *font_family;            // NULL -> system default
    const char *font_family_bold;       // NULL -> derive
    const char *font_family_italic;
    const char *font_family_bold_italic;
    const char *font_features;          // comma-separated: "ss01,liga"
    bool        font_thicken;
    double      content_scale;          // 0 -> 1.0
} GhosttyFontGridConfig;

Renderer API

// Lifecycle
GhosttyRendererResult ghostty_renderer_new(GhosttyFontGrid grid,
                                            const GhosttyRendererConfig *config,
                                            GhosttyRenderer *out);
void ghostty_renderer_free(GhosttyRenderer r);

// Terminal binding (terminal is from libghostty-vt; NULL unbinds)
void ghostty_renderer_set_terminal(GhosttyRenderer r, GhosttyTerminal t);

// Surface layout (for grid/padding calculations, NOT a GPU surface)
void ghostty_renderer_resize(GhosttyRenderer r, uint32_t width_px, uint32_t height_px);

// Frame generation (locks terminal briefly, shapes text, fills buffers)
GhosttyRendererResult ghostty_renderer_update_frame(GhosttyRenderer r,
                                                     bool cursor_blink_visible);

// Cell buffer output (pointers valid until next update_frame)
const GhosttyRendererCellBg   *ghostty_renderer_bg_cells(GhosttyRenderer r, uint32_t *count);
const GhosttyRendererCellText *ghostty_renderer_text_cells(GhosttyRenderer r, uint32_t *count);
void ghostty_renderer_frame_data(GhosttyRenderer r, GhosttyRendererFrameData *out);

// Runtime config
void ghostty_renderer_set_background(GhosttyRenderer r, GhosttyRendererRGB color);
void ghostty_renderer_set_foreground(GhosttyRenderer r, GhosttyRendererRGB color);
void ghostty_renderer_set_background_opacity(GhosttyRenderer r, float opacity);
void ghostty_renderer_set_min_contrast(GhosttyRenderer r, float contrast);
void ghostty_renderer_set_palette(GhosttyRenderer r, const GhosttyRendererRGB palette[256]);

// Theme
GhosttyRendererResult ghostty_renderer_load_theme(GhosttyRenderer r, const char *name);
GhosttyRendererResult ghostty_renderer_load_theme_file(GhosttyRenderer r, const char *path);

Output types

typedef uint8_t GhosttyRendererCellBg[4];  // RGBA, [rows*cols] row-major

typedef struct {                           // 32 bytes, matches Ghostty's internal CellText
    uint32_t glyph_pos[2];                 // atlas pixel position
    uint32_t glyph_size[2];                // glyph pixel size
    int16_t  bearings[2];                  // font bearings
    uint16_t grid_pos[2];                  // cell (col, row)
    uint8_t  color[4];                     // RGBA
    uint8_t  atlas;                        // 0=grayscale, 1=color
    uint8_t  flags;                        // bit 0: no_min_contrast, bit 1: is_cursor_glyph
    uint8_t  _pad[2];
} GhosttyRendererCellText;

typedef struct {
    float    cell_width, cell_height;
    uint16_t grid_cols, grid_rows;
    float    grid_padding[4];              // left, top, right, bottom
    uint8_t  bg_color[4];
    float    min_contrast;
    uint16_t cursor_pos[2];
    uint8_t  cursor_color[4];
    bool     cursor_wide, cursor_visible, cursor_blinking;
    GhosttyRendererCursorStyle cursor_style;
    bool     has_selection;
    bool     password_input;
} GhosttyRendererFrameData;

What's removed vs current fork

┌────────────────────────────────────┬─────────────────────────────────────────┐
│            Current fork            │               New design                │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ ghostty_terminal_new/free/vt_write │ Gone. Use libghostty-vt.                │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ ghostty_terminal_resize/scroll     │ Gone. Use libghostty-vt.                │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ ghostty_terminal_dump_screen       │ Gone. Use libghostty-vt.                │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ ghostty_terminal_mode_*            │ Gone. Use libghostty-vt.                │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ ghostty_renderer_draw_frame (L1)   │ Gone. No Level 1.                       │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ Single RendererState handle        │ Split: FontGrid + Renderer.             │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ Flat CConfig struct                │ Split: FontGridConfig + RendererConfig. │
├────────────────────────────────────┼─────────────────────────────────────────┤
│ Stub theme/font-size functions     │ Implemented.                            │
└────────────────────────────────────┴─────────────────────────────────────────┘

---
Internal Zig changes

1. Bridge: GhosttyTerminal -> *terminal.Terminal

The renderer needs *terminal.Terminal (Zig struct) but receives GhosttyTerminal
(opaque pointer to libghostty-vt's TerminalWrapper).

Both libraries compile from the same Ghostty source, so TerminalWrapper layout is
identical. The renderer casts GhosttyTerminal -> *TerminalWrapper and reads .terminal.
This is what the current lib_renderer_c.zig:224 already does.

For a cleaner upstream path, also add to libghostty-vt:
// in vt/terminal.h — returns the raw internal terminal pointer
GHOSTTY_API void* ghostty_terminal_raw_ptr(GhosttyTerminal t);

2. Split RendererState into FontGridState + RendererState

Current lib_renderer_c.zig:73:
const RendererState = struct {
    alloc, renderer, render_state, terminal_set,
    font_grid_set, font_grid, font_grid_key,
    config, mutex, text_cells_staging, atlas_*_gen
};

New:
const FontGridState = struct {
    alloc: Allocator,
    font_grid_set: font.SharedGridSet,
    font_grid: *font.SharedGrid,
    font_grid_key: font.SharedGridSet.Key,
    config: configpkg.Config,           // font-related config
    atlas_grayscale_gen: usize = 0,
    atlas_color_gen: usize = 0,
};

const RendererState = struct {
    alloc: Allocator,
    renderer: Renderer,                 // generic.zig instance
    render_state: rendererpkg.State,
    font_grid_state: *FontGridState,    // borrowed, not owned
    terminal_set: bool = false,
    mutex: std.Thread.Mutex = .{},
    text_cells_staging: []CellText = &.{},
    renderer_config: RendererDerivedConfig,
};

3. Headless renderer init

The current fork uses Metal backend with view_info = null. This works because:
- updateFrame / rebuildCells only use font shaper + terminal state (no GPU)
- Atlas textures are populated in CPU memory by SharedGrid
- Cell buffers are plain arrays in self.cells

For headless (no native view), Metal.init needs to handle view_info = null gracefully.
The current fork's Options.zig changes (making rt_surface, surface_mailbox, thread
optional) already accomplish this. The Metal.zig change (line 96-101) provides the
view_info fallback path.

No additional headless backend needed for macOS. The consumer never calls drawFrame.

4. Enrich FrameData

After updateFrame, plumb additional state from self.terminal_state:
- cursor.visible, cursor.blinking, cursor.visual_style, cursor.password_input
- has_selection: scan row_data.items(.selection) for any non-null
- These are trivial reads from existing state, no new computation needed.

5. Implement theme loading

ghostty_renderer_load_theme:
1. Resolve theme name via global.state.resources_dir
2. Parse the theme file (ghostty config format)
3. Extract palette + fg/bg/cursor colors
4. Apply to renderer.config and renderer.uniforms
5. Mark full dirty so next update_frame rebuilds all cells

Reference: src/config/Config.zig already has theme loading via loadTheme().

6. Implement font size change

ghostty_font_grid_set_size:
1. Deref old SharedGrid via font_grid_set.deref(old_key)
2. Create new DerivedConfig with new point size + DPI
3. Ref new grid: font_grid_set.ref(&new_config, new_size)
4. Update any renderers using this grid (via callback or invalidation flag)
5. Consumer must re-query metrics and re-upload atlas

---
Build system

GhosttyLibRenderer.zig changes

Follow GhosttyLibVt.zig patterns:

pub fn initStatic(b, deps) !GhosttyLibRenderer {
    const lib = b.addLibrary(.{
        .name = "ghostty-renderer-static",
        .linkage = .static,
        .root_module = deps.renderer_c,   // not b.createModule
    });
    lib.bundle_compiler_rt = true;
    lib.root_module.pic = true;
    lib.installHeadersDirectory(b.path("include/ghostty"), "ghostty",
        .{ .include_extensions = &.{".h"} });

    // SIMD fat archive (macOS libtool, Linux ar -M)
    // pkg-config: libghostty-renderer.pc
    // DSym on macOS
}

build.zig addition

if (!config.target.result.cpu.arch.isWasm()) {
    const renderer_lib = try buildpkg.GhosttyLibRenderer.initStatic(b, &deps);
    renderer_lib.install();
}

---
Upstream PR sequence

PR 1: Standalone renderer options (~50 lines)

Files: Options.zig, Metal.zig, generic.zig
Already done in fork. Makes rt_surface, surface_mailbox, thread optional.
Smallest, most upstreamable change. Enables renderer use without apprt.

PR 2: ghostty_terminal_raw_ptr in libghostty-vt (~20 lines)

Files: src/terminal/c/terminal.zig, include/ghostty/vt/terminal.h
Returns inner *Terminal from wrapper. Enables clean cross-library handle passing.

PR 3: Build target for libghostty-renderer (~100 lines)

Files: src/build/GhosttyLibRenderer.zig, build.zig
Static library target with pkg-config, fat archive, header install.

PR 4: Font grid C API (~200 lines)

Files: src/lib_renderer_c.zig (partial), include/ghostty/renderer.h (partial)
ghostty_font_grid_new/free/get_metrics/atlas_grayscale/atlas_color/set_size

PR 5: Renderer C API (~400 lines)

Files: src/lib_renderer_c.zig, src/lib_renderer.zig, include/ghostty/renderer.h
ghostty_renderer_new/free/set_terminal/resize/update_frame/bg_cells/text_cells/frame_data
Plus runtime config setters and theme loading.

PR 6: Example (c-renderer-level2) (~150 lines)

Files: example/c-renderer-level2/
Minimal C program: create terminal (vt), create font grid, create renderer, bind terminal,
update frame, read cell buffers, print stats. Proves the API end-to-end.

---
Rust consumer changes

ghostty-renderer-sys (FFI bindings)

Replace current bindings with new API. build.rs links libghostty-renderer.a and
libghostty-vt.a (both needed). Generates Rust types for all C structs.

ghostty-renderer (safe wrapper)

pub struct FontGrid { raw: NonNull<c_void> }
pub struct Renderer { raw: NonNull<c_void>, _grid: Arc<FontGrid> }

impl FontGrid {
    fn new(config: &FontGridConfig) -> Result<Self>;
    fn metrics(&self) -> FontMetrics;
    fn atlas_grayscale(&self) -> (&[u8], u32, bool);  // data, size, modified
    fn atlas_color(&self) -> (&[u8], u32, bool);
    fn set_size(&self, points: f32) -> Result<()>;
}

impl Renderer {
    fn new(grid: Arc<FontGrid>, config: &RendererConfig) -> Result<Self>;
    fn set_terminal(&self, t: &vt::Terminal);
    fn resize(&self, w: u32, h: u32);
    fn update_frame(&self, cursor_blink: bool) -> Result<()>;
    fn bg_cells(&self) -> &[[u8; 4]];
    fn text_cells(&self) -> &[CellText];
    fn frame_data(&self) -> FrameData;
}

seance-terminal

struct App {
    font_grid: Arc<FontGrid>,     // shared across all panes
    panes: HashMap<PaneId, Pane>,
}

struct Pane {
    terminal: vt::Terminal,       // from libghostty-vt
    renderer: Renderer,           // from libghostty-renderer, shares font_grid
    pty: Pty,
}

One atlas upload per frame (shared). Per-pane: update_frame + buffer upload + draw call
with pane-specific viewport scissor rect.

---
Verification

1. Build ghostty with renderer library: zig build produces libghostty-renderer.a
2. Build seance with updated Rust crates: cargo build
3. Run seance, verify terminal rendering works (text, colors, cursor, selection)
4. Test multiplexer: create splits, verify each pane renders independently
5. Test font size change: call set_size, verify atlas re-upload and correct rendering
6. Test theme loading: call load_theme("Catppuccin Frappe"), verify colors update


Use https://github.com/Uzaaft/libghostty-rs
