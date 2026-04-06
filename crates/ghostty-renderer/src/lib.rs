//! Safe Rust wrapper for libghostty-renderer.
//!
//! `FontGrid` (shared font atlas) and `Renderer` (per-terminal cell
//! buffer generator). Terminal emulation comes from libghostty-vt.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::rc::Rc;

use ghostty_renderer_sys as ffi;

pub use ffi::GhosttyFontGridConfig as FontGridConfig;
pub use ffi::GhosttyRendererConfig as RendererConfig;
pub use ffi::GhosttyFontMetrics as FontMetrics;
pub use ffi::GhosttyRendererCellText as CellText;
pub use ffi::GhosttyRendererFrameData as FrameData;
pub use ffi::GhosttyBlending as Blending;
pub use ffi::GhosttyColorspace as Colorspace;
pub use ffi::GhosttyPaddingColor as PaddingColor;
pub use ffi::GhosttyRendererOptRGB as OptColor;
pub use ffi::GhosttyRendererRGB as Color;

pub struct FontGrid {
    raw: NonNull<c_void>,
    _not_send: PhantomData<*mut ()>,
}

impl FontGrid {
    pub fn new(config: &FontGridConfig) -> Option<Self> {
        let raw = unsafe { ffi::ghostty_font_grid_new(config) };
        NonNull::new(raw).map(|raw| Self { raw, _not_send: PhantomData })
    }

    pub fn metrics(&self) -> FontMetrics {
        let mut out = FontMetrics::default();
        unsafe { ffi::ghostty_font_grid_get_metrics(self.raw.as_ptr(), &mut out) };
        out
    }

    pub fn atlas_grayscale(&self) -> AtlasTexture<'_> {
        let mut size: u32 = 0;
        let mut modified = false;
        let ptr = unsafe {
            ffi::ghostty_font_grid_atlas_grayscale(self.raw.as_ptr(), &mut size, &mut modified)
        };
        let len = (size as usize) * (size as usize);
        let data = if len == 0 || ptr.is_null() { &[] }
            else { unsafe { std::slice::from_raw_parts(ptr, len) } };
        AtlasTexture { data, size, modified }
    }

    pub fn atlas_color(&self) -> AtlasTexture<'_> {
        let mut size: u32 = 0;
        let mut modified = false;
        let ptr = unsafe {
            ffi::ghostty_font_grid_atlas_color(self.raw.as_ptr(), &mut size, &mut modified)
        };
        let len = (size as usize) * (size as usize) * 4;
        let data = if len == 0 || ptr.is_null() { &[] }
            else { unsafe { std::slice::from_raw_parts(ptr, len) } };
        AtlasTexture { data, size, modified }
    }

    pub fn set_size(&self, points: f32) {
        unsafe { ffi::ghostty_font_grid_set_size(self.raw.as_ptr(), points) };
    }
}

impl Drop for FontGrid {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_font_grid_free(self.raw.as_ptr()) };
    }
}

pub struct Renderer {
    raw: NonNull<c_void>,
    _grid: Rc<FontGrid>,
    _not_send: PhantomData<*mut ()>,
}

impl Renderer {
    pub fn new(grid: Rc<FontGrid>, config: &RendererConfig) -> Option<Self> {
        let raw = unsafe { ffi::ghostty_renderer_new(grid.raw.as_ptr(), config) };
        NonNull::new(raw).map(|raw| Self { raw, _grid: grid, _not_send: PhantomData })
    }

    /// # Safety
    /// `terminal` must be a valid GhosttyTerminal handle from libghostty-vt.
    pub unsafe fn set_terminal_raw(&self, terminal: *mut c_void) {
        unsafe { ffi::ghostty_renderer_set_terminal(self.raw.as_ptr(), terminal) };
    }

    pub fn resize(&self, width: u32, height: u32) {
        unsafe { ffi::ghostty_renderer_resize(self.raw.as_ptr(), width, height) };
    }

    pub fn update_frame(&self, cursor_blink_visible: bool) {
        unsafe { ffi::ghostty_renderer_update_frame(self.raw.as_ptr(), cursor_blink_visible) };
    }

    pub fn frame_snapshot(&self) -> FrameSnapshot<'_> {
        FrameSnapshot { renderer: self }
    }

    pub fn load_theme(&self, name: &std::ffi::CStr) -> bool {
        unsafe { ffi::ghostty_renderer_load_theme(self.raw.as_ptr(), name.as_ptr()) }
    }

    pub fn load_theme_file(&self, path: &std::ffi::CStr) -> bool {
        unsafe { ffi::ghostty_renderer_load_theme_file(self.raw.as_ptr(), path.as_ptr()) }
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

    pub fn set_palette(&self, palette: &[Color; 256]) {
        unsafe { ffi::ghostty_renderer_set_palette(self.raw.as_ptr(), palette.as_ptr()) };
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_renderer_free(self.raw.as_ptr()) };
    }
}

pub struct FrameSnapshot<'r> {
    renderer: &'r Renderer,
}

impl<'r> FrameSnapshot<'r> {
    pub fn bg_cells(&self) -> &[CellBg] {
        let mut count: u32 = 0;
        let ptr = unsafe { ffi::ghostty_renderer_bg_cells(self.renderer.raw.as_ptr(), &mut count) };
        if count == 0 || ptr.is_null() { return &[]; }
        unsafe { std::slice::from_raw_parts(ptr.cast(), count as usize) }
    }

    pub fn text_cells(&self) -> &[CellText] {
        let mut count: u32 = 0;
        let ptr = unsafe { ffi::ghostty_renderer_text_cells(self.renderer.raw.as_ptr(), &mut count) };
        if count == 0 || ptr.is_null() { return &[]; }
        unsafe { std::slice::from_raw_parts(ptr, count as usize) }
    }

    pub fn frame_data(&self) -> FrameData {
        let mut data: FrameData = unsafe { std::mem::zeroed() };
        unsafe { ffi::ghostty_renderer_frame_data(self.renderer.raw.as_ptr(), &mut data) };
        data
    }

    pub fn atlas_grayscale(&self) -> AtlasTexture<'_> {
        self.renderer._grid.atlas_grayscale()
    }

    pub fn atlas_color(&self) -> AtlasTexture<'_> {
        self.renderer._grid.atlas_color()
    }
}

pub type CellBg = [u8; 4];

pub struct AtlasTexture<'a> {
    pub data: &'a [u8],
    pub size: u32,
    pub modified: bool,
}
