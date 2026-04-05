//! Safe Rust wrapper for libghostty-renderer.
//!
//! Provides `Renderer` and `Terminal` types that manage the lifecycle
//! of their underlying C handles. All unsafe FFI calls are encapsulated
//! with documented safety invariants.
//!
//! # Thread Safety
//!
//! Neither `Renderer` nor `Terminal` are thread-safe. They must be used
//! from a single thread (typically the main/render thread). This matches
//! the threading model of the underlying C library.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;

use ghostty_renderer_sys as ffi;

pub use ffi::GhosttyRendererConfig as RendererConfig;

// Re-export vertex data types for consumers building their own GPU pipeline.
pub use ffi::GhosttyRendererCellText as CellText;
pub use ffi::GhosttyRendererFrameData as FrameData;

// Re-export enums and basic types so consumers don't need `ghostty_renderer_sys`.
pub use ffi::GhosttyBlending as Blending;
pub use ffi::GhosttyColor as Color;
pub use ffi::GhosttyColorspace as Colorspace;
pub use ffi::GhosttyOptColor as OptColor;
pub use ffi::GhosttyPaddingColor as PaddingColor;
pub use ffi::GhosttyScrollAction;

/// GPU terminal renderer backed by libghostty-renderer.
///
/// Manages font loading, text shaping, glyph atlas, and cell buffer
/// generation. Can either draw to a platform surface (Level 1) or
/// provide GPU-ready buffers for the consumer to draw (Level 2).
pub struct Renderer {
    raw: NonNull<c_void>,
    // Not Send/Sync: the underlying C state is single-threaded.
    _not_send: PhantomData<*mut ()>,
}

impl Renderer {
    /// Create a new renderer.
    ///
    /// `native_handle` is a platform-specific window handle (NSView* on macOS).
    /// Pass null for headless / Level 2 only use.
    pub fn new(config: &RendererConfig, native_handle: *mut c_void) -> Option<Self> {
        // SAFETY: `config` is a valid pointer to an initialized struct.
        // `native_handle` may be null (for headless use). The C function
        // returns null on failure, which we check below.
        let raw = unsafe { ffi::ghostty_renderer_new(config, native_handle) };
        NonNull::new(raw).map(|raw| Self {
            raw,
            _not_send: PhantomData,
        })
    }

    /// Associate a terminal with this renderer.
    ///
    /// The terminal must remain valid for the lifetime of the renderer
    /// or until a different terminal is set. This is enforced by requiring
    /// a reference with an appropriate lifetime.
    pub fn set_terminal<'a>(&'a self, terminal: &'a Terminal) {
        // SAFETY: both `self.raw` and `terminal.raw` are valid, non-null
        // handles. The borrow checker ensures `terminal` lives at least
        // as long as `self` through the shared lifetime `'a`.
        unsafe { ffi::ghostty_renderer_set_terminal(self.raw.as_ptr(), terminal.raw.as_ptr()) };
    }

    /// Notify the renderer of a surface resize.
    pub fn resize(&self, width: u32, height: u32, scale: f64) {
        // SAFETY: `self.raw` is a valid handle.
        unsafe { ffi::ghostty_renderer_resize(self.raw.as_ptr(), width, height, scale) };
    }

    // -- Level 1: library draws to bound surface --

    /// Rebuild cell buffers from the associated terminal's state.
    pub fn update_frame(&self) {
        // SAFETY: `self.raw` is a valid handle. If no terminal is set,
        // the C function returns immediately.
        unsafe { ffi::ghostty_renderer_update_frame(self.raw.as_ptr()) };
    }

    /// Draw the current frame to the bound surface.
    pub fn draw_frame(&self) {
        // SAFETY: `self.raw` is a valid handle.
        unsafe { ffi::ghostty_renderer_draw_frame(self.raw.as_ptr()) };
    }

    // -- Level 2: consumer reads cell buffers --

    /// Snapshot of GPU-ready data produced by `update_frame`.
    ///
    /// Call `update_frame()` first, then use `frame_snapshot()` to get
    /// an immutable view of the cell buffers and atlas textures.
    /// The snapshot borrows the renderer, preventing mutation
    /// (including another `update_frame`) while it is alive.
    pub fn frame_snapshot(&self) -> FrameSnapshot<'_> {
        FrameSnapshot { renderer: self }
    }

    // -- Theme --

    /// Load a named theme (searches Ghostty's built-in theme list).
    pub fn load_theme(&self, name: &std::ffi::CStr) -> bool {
        unsafe { ffi::ghostty_renderer_load_theme(self.raw.as_ptr(), name.as_ptr()) }
    }

    /// Load a theme from a file path.
    pub fn load_theme_file(&self, path: &std::ffi::CStr) -> bool {
        unsafe { ffi::ghostty_renderer_load_theme_file(self.raw.as_ptr(), path.as_ptr()) }
    }

    // -- Runtime config --

    pub fn set_font_size(&self, points: f32) {
        unsafe { ffi::ghostty_renderer_set_font_size(self.raw.as_ptr(), points) };
    }

    pub fn set_background(&self, color: Color) {
        unsafe { ffi::ghostty_renderer_set_background(self.raw.as_ptr(), color.r, color.g, color.b) };
    }

    pub fn set_foreground(&self, color: Color) {
        unsafe { ffi::ghostty_renderer_set_foreground(self.raw.as_ptr(), color.r, color.g, color.b) };
    }

    pub fn set_background_opacity(&self, opacity: f32) {
        unsafe { ffi::ghostty_renderer_set_background_opacity(self.raw.as_ptr(), opacity) };
    }

    pub fn set_min_contrast(&self, contrast: f32) {
        unsafe { ffi::ghostty_renderer_set_min_contrast(self.raw.as_ptr(), contrast) };
    }

    /// Set the 256-color palette. `palette` must contain exactly 256 entries.
    pub fn set_palette(&self, palette: &[Color; 256]) {
        unsafe { ffi::ghostty_renderer_set_palette(self.raw.as_ptr(), palette.as_ptr()) };
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: `self.raw` was allocated by `ghostty_renderer_new`
        // and has not been freed (we only free in Drop, which runs once).
        unsafe { ffi::ghostty_renderer_free(self.raw.as_ptr()) };
    }
}

/// Immutable view of the renderer's cell buffers and atlas textures.
///
/// Borrows the `Renderer`, preventing `update_frame()` or `draw_frame()`
/// from being called while the snapshot is alive. This ensures all
/// returned slices remain valid.
pub struct FrameSnapshot<'r> {
    renderer: &'r Renderer,
}

impl<'r> FrameSnapshot<'r> {
    /// Background colors: flat `[rows * cols]` array of RGBA values.
    pub fn bg_cells(&self) -> &[CellBg] {
        let mut count: u32 = 0;
        // SAFETY: `self.renderer.raw` is valid. The returned pointer is
        // owned by the renderer and valid until the next `update_frame`.
        // The borrow of `&self` (which borrows the renderer) prevents that.
        let ptr = unsafe {
            ffi::ghostty_renderer_get_bg_cells(self.renderer.raw.as_ptr(), &mut count)
        };
        if count == 0 || ptr.is_null() {
            return &[];
        }
        // SAFETY: `ptr` points to `count` contiguous `[u8; 4]` values
        // owned by the renderer. Valid for the lifetime of `self`.
        unsafe { std::slice::from_raw_parts(ptr.cast(), count as usize) }
    }

    /// Foreground glyph instances (text, underlines, cursors).
    pub fn text_cells(&self) -> &[CellText] {
        let mut count: u32 = 0;
        // SAFETY: same invariants as `bg_cells`.
        let ptr = unsafe {
            ffi::ghostty_renderer_get_text_cells(self.renderer.raw.as_ptr(), &mut count)
        };
        if count == 0 || ptr.is_null() {
            return &[];
        }
        // SAFETY: `ptr` points to `count` contiguous `CellText` values
        // in the renderer's staging buffer, valid for the lifetime of `self`.
        unsafe { std::slice::from_raw_parts(ptr, count as usize) }
    }

    /// Per-frame layout and color parameters.
    pub fn frame_data(&self) -> FrameData {
        // SAFETY: `zeroed` is valid for `FrameData` (all-zero is a valid
        // bit pattern for this POD struct of integers, floats, and bools).
        let mut data: FrameData = unsafe { std::mem::zeroed() };
        // SAFETY: `self.renderer.raw` is valid. `data` is a valid output pointer.
        unsafe {
            ffi::ghostty_renderer_get_frame_data(self.renderer.raw.as_ptr(), &mut data);
        }
        data
    }

    /// Glyph atlas (grayscale, 1 byte/pixel, for text).
    ///
    /// Returns `(pixel_data, texture_size, modified_since_last_call)`.
    /// The texture is square: `texture_size x texture_size` pixels.
    pub fn atlas_grayscale(&self) -> AtlasTexture<'_> {
        let mut size: u32 = 0;
        let mut modified = false;
        // SAFETY: `self.renderer.raw` is valid. Output pointers are valid.
        let ptr = unsafe {
            ffi::ghostty_renderer_get_atlas_grayscale(
                self.renderer.raw.as_ptr(),
                &mut size,
                &mut modified,
            )
        };
        let len = (size as usize) * (size as usize);
        let data = if len == 0 || ptr.is_null() {
            &[]
        } else {
            // SAFETY: atlas data is owned by the font grid, protected by
            // a shared lock during the call. Valid for lifetime of `self`.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        AtlasTexture {
            data,
            size,
            modified,
        }
    }

    /// Glyph atlas (BGRA, 4 bytes/pixel, for color emoji).
    ///
    /// Returns `(pixel_data, texture_size, modified_since_last_call)`.
    /// The texture is square: `texture_size x texture_size` pixels.
    pub fn atlas_color(&self) -> AtlasTexture<'_> {
        let mut size: u32 = 0;
        let mut modified = false;
        // SAFETY: same invariants as `atlas_grayscale`.
        let ptr = unsafe {
            ffi::ghostty_renderer_get_atlas_color(
                self.renderer.raw.as_ptr(),
                &mut size,
                &mut modified,
            )
        };
        let bytes_per_pixel = 4; // BGRA
        let len = (size as usize) * (size as usize) * bytes_per_pixel;
        let data = if len == 0 || ptr.is_null() {
            &[]
        } else {
            // SAFETY: same as `atlas_grayscale`, accounting for BGRA stride.
            unsafe { std::slice::from_raw_parts(ptr, len) }
        };
        AtlasTexture {
            data,
            size,
            modified,
        }
    }
}

/// A single cell's background color (RGBA, 1 byte per channel).
pub type CellBg = [u8; 4];

/// A glyph atlas texture snapshot.
pub struct AtlasTexture<'a> {
    pub data: &'a [u8],
    /// Side length of the square texture in pixels.
    pub size: u32,
    /// Whether the atlas has been modified since the last snapshot.
    pub modified: bool,
}

// ============================================================
// Terminal
// ============================================================

/// Scroll direction for `Terminal::scroll`.
#[derive(Debug, Clone, Copy)]
pub enum ScrollAction {
    Lines(i32),
    Top,
    Bottom,
    PageUp,
    PageDown,
}

impl ScrollAction {
    fn to_ffi(self) -> (GhosttyScrollAction, i32) {
        match self {
            ScrollAction::Lines(d) => (GhosttyScrollAction::Lines, d),
            ScrollAction::Top => (GhosttyScrollAction::Top, 0),
            ScrollAction::Bottom => (GhosttyScrollAction::Bottom, 0),
            ScrollAction::PageUp => (GhosttyScrollAction::PageUp, 0),
            ScrollAction::PageDown => (GhosttyScrollAction::PageDown, 0),
        }
    }
}

/// Cursor state returned by `Terminal::cursor`.
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

/// Terminal emulator backed by libghostty-vt.
///
/// Manages VT parsing, terminal state, scrollback, and
/// input encoding. Not thread-safe.
pub struct Terminal {
    raw: NonNull<c_void>,
    _not_send: PhantomData<*mut ()>,
}

impl Terminal {
    /// Create a new terminal with the given grid dimensions.
    pub fn new(cols: u16, rows: u16) -> Option<Self> {
        // SAFETY: the C function allocates and initializes a terminal.
        // Returns null on failure.
        let raw = unsafe { ffi::ghostty_terminal_new(cols, rows, 10_000) };
        NonNull::new(raw).map(|raw| Self {
            raw,
            _not_send: PhantomData,
        })
    }

    /// Feed raw bytes (PTY output) into the VT parser.
    pub fn vt_write(&mut self, data: &[u8]) {
        // SAFETY: `self.raw` is valid. `data` is a valid slice.
        // The C function reads `len` bytes from `data.as_ptr()`.
        unsafe { ffi::ghostty_terminal_vt_write(self.raw.as_ptr(), data.as_ptr(), data.len()) };
    }

    /// Resize the terminal grid.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        // SAFETY: `self.raw` is valid.
        unsafe { ffi::ghostty_terminal_resize(self.raw.as_ptr(), cols, rows) };
    }

    /// Drain response bytes that the terminal wants to send back to the PTY
    /// (e.g. device attribute replies, size reports).
    ///
    /// Returns an empty slice if there are no pending responses.
    /// The returned data is valid until the next mutating call (`vt_write`,
    /// `resize`, `clear_responses`). Taking `&mut self` prevents aliasing.
    pub fn drain_responses(&mut self) -> &[u8] {
        let mut len: usize = 0;
        // SAFETY: `self.raw` is valid. `len` is a valid output pointer.
        let ptr = unsafe { ffi::ghostty_terminal_drain_responses(self.raw.as_ptr(), &mut len) };
        if len == 0 || ptr.is_null() {
            return &[];
        }
        // SAFETY: the C function returns a pointer to `len` bytes in the
        // terminal's response buffer. Valid for `'_` (the reborrow of `self`).
        unsafe { std::slice::from_raw_parts(ptr, len) }
    }

    /// Clear the response buffer.
    pub fn clear_responses(&mut self) {
        // SAFETY: `self.raw` is valid.
        unsafe { ffi::ghostty_terminal_clear_responses(self.raw.as_ptr()) };
    }

    /// Get the current grid dimensions.
    pub fn size(&self) -> (u16, u16) {
        let (mut cols, mut rows) = (0u16, 0u16);
        // SAFETY: `self.raw` is valid. Output pointers are valid.
        unsafe { ffi::ghostty_terminal_get_size(self.raw.as_ptr(), &mut cols, &mut rows) };
        (cols, rows)
    }

    /// Get the current cursor position and visibility.
    pub fn cursor(&self) -> CursorState {
        let (mut col, mut row) = (0u16, 0u16);
        let mut visible = false;
        // SAFETY: `self.raw` is valid. Output pointers are valid.
        unsafe {
            ffi::ghostty_terminal_get_cursor(self.raw.as_ptr(), &mut col, &mut row, &mut visible);
        }
        CursorState { col, row, visible }
    }

    /// Scroll the terminal viewport.
    pub fn scroll(&mut self, action: ScrollAction) {
        let (ffi_action, delta) = action.to_ffi();
        unsafe { ffi::ghostty_terminal_scroll(self.raw.as_ptr(), ffi_action, delta) };
    }

    /// Number of scrollback rows currently available.
    pub fn scrollback_rows(&self) -> usize {
        unsafe { ffi::ghostty_terminal_get_scrollback_rows(self.raw.as_ptr()) }
    }

    /// Get the terminal title (set via OSC 0/2).
    pub fn title(&self) -> Option<String> {
        let mut len: usize = 0;
        let ptr = unsafe { ffi::ghostty_terminal_get_title(self.raw.as_ptr(), &mut len) };
        if ptr.is_null() || len == 0 {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    /// Whether DECCKM (application cursor keys) is active.
    pub fn mode_cursor_keys(&self) -> bool {
        unsafe { ffi::ghostty_terminal_mode_cursor_keys(self.raw.as_ptr()) }
    }

    /// Active mouse tracking mode (0=none, 9=X10, 1000/1002/1003).
    pub fn mode_mouse_event(&self) -> i32 {
        unsafe { ffi::ghostty_terminal_mode_mouse_event(self.raw.as_ptr()) }
    }

    /// Whether SGR mouse format (mode 1006) is active.
    pub fn mode_mouse_format_sgr(&self) -> bool {
        unsafe { ffi::ghostty_terminal_mode_mouse_format_sgr(self.raw.as_ptr()) }
    }

    /// Whether synchronized output (mode 2026) is active.
    pub fn mode_synchronized_output(&self) -> bool {
        unsafe { ffi::ghostty_terminal_mode_synchronized_output(self.raw.as_ptr()) }
    }

    /// Dump the current screen content as UTF-8 text.
    pub fn dump_screen(&mut self) -> String {
        let mut buf = vec![0u8; 64 * 1024];
        let len = unsafe {
            ffi::ghostty_terminal_dump_screen(self.raw.as_ptr(), buf.as_mut_ptr(), buf.len())
        };
        buf.truncate(len);
        String::from_utf8_lossy(&buf).into_owned()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // SAFETY: `self.raw` was allocated by `ghostty_terminal_new`
        // and has not been freed.
        unsafe { ffi::ghostty_terminal_free(self.raw.as_ptr()) };
    }
}
