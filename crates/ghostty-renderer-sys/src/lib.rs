//! Raw FFI bindings to libghostty-renderer.

#![allow(non_camel_case_types)]

use std::ffi::c_void;

// ================================================================
// Handles
// ================================================================

pub type GhosttyFontGrid = *mut c_void;
pub type GhosttyRenderer = *mut c_void;

// ================================================================
// Basic types
// ================================================================

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyRendererRGB {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyRendererOptRGB {
    pub color: GhosttyRendererRGB,
    pub has: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub enum GhosttyColorspace {
    #[default]
    Srgb = 0,
    DisplayP3 = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub enum GhosttyBlending {
    #[default]
    Native = 0,
    Linear = 1,
    LinearCorrected = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub enum GhosttyPaddingColor {
    #[default]
    Background = 0,
    Extend = 1,
    ExtendAlways = 2,
}

// ================================================================
// Font grid config
// ================================================================

#[repr(C)]
#[derive(Default)]
pub struct GhosttyFontGridConfig {
    pub font_size: f32,
    pub font_family: *const i8,
    pub font_family_bold: *const i8,
    pub font_family_italic: *const i8,
    pub font_family_bold_italic: *const i8,
    pub font_features: *const i8,
    pub font_thicken: bool,
    pub content_scale: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyFontMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub cell_baseline: f32,
    pub underline_position: f32,
    pub underline_thickness: f32,
    pub strikethrough_position: f32,
    pub strikethrough_thickness: f32,
}

// ================================================================
// Renderer config
// ================================================================

#[repr(C)]
#[derive(Default)]
pub struct GhosttyRendererConfig {
    pub width_px: u32,
    pub height_px: u32,
    pub content_scale: f64,
    pub native_view: *mut c_void,

    pub background: GhosttyRendererRGB,
    pub foreground: GhosttyRendererRGB,
    pub cursor_color: GhosttyRendererOptRGB,
    pub cursor_text: GhosttyRendererOptRGB,
    pub selection_background: GhosttyRendererOptRGB,
    pub selection_foreground: GhosttyRendererOptRGB,
    pub bold_color: GhosttyRendererOptRGB,

    pub search_match_bg: GhosttyRendererRGB,
    pub search_match_fg: GhosttyRendererRGB,
    pub search_selected_bg: GhosttyRendererRGB,
    pub search_selected_fg: GhosttyRendererRGB,

    pub background_opacity: f32,
    pub cursor_opacity: f32,
    pub faint_opacity: f32,

    pub min_contrast: f32,
    pub colorspace: GhosttyColorspace,
    pub alpha_blending: GhosttyBlending,
    pub padding_color: GhosttyPaddingColor,
}

// ================================================================
// Cell buffer types
// ================================================================

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GhosttyRendererCellBg {
    pub rgba: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GhosttyRendererCellText {
    pub glyph_pos: [u32; 2],
    pub glyph_size: [u32; 2],
    pub bearings: [i16; 2],
    pub grid_pos: [u16; 2],
    pub color: [u8; 4],
    pub atlas: u8,
    pub flags: u8,
    pub _pad: [u8; 2],
}

const _: () = assert!(size_of::<GhosttyRendererCellText>() == 32);

#[repr(C)]
pub struct GhosttyRendererFrameData {
    pub cell_width: f32,
    pub cell_height: f32,
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub grid_padding: [f32; 4],
    pub bg_color: [u8; 4],
    pub min_contrast: f32,
    pub cursor_pos: [u16; 2],
    pub cursor_color: [u8; 4],
    pub cursor_wide: bool,
}

// ================================================================
// FFI
// ================================================================

unsafe extern "C" {
    // Font grid
    pub fn ghostty_font_grid_new(config: *const GhosttyFontGridConfig) -> GhosttyFontGrid;
    pub fn ghostty_font_grid_free(grid: GhosttyFontGrid);
    pub fn ghostty_font_grid_get_metrics(grid: GhosttyFontGrid, out: *mut GhosttyFontMetrics);
    pub fn ghostty_font_grid_atlas_grayscale(
        grid: GhosttyFontGrid,
        size: *mut u32,
        modified: *mut bool,
    ) -> *const u8;
    pub fn ghostty_font_grid_atlas_color(
        grid: GhosttyFontGrid,
        size: *mut u32,
        modified: *mut bool,
    ) -> *const u8;
    pub fn ghostty_font_grid_set_size(grid: GhosttyFontGrid, points: f32);

    // Renderer
    pub fn ghostty_renderer_new(
        grid: GhosttyFontGrid,
        config: *const GhosttyRendererConfig,
    ) -> GhosttyRenderer;
    pub fn ghostty_renderer_free(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_set_terminal(renderer: GhosttyRenderer, terminal: *mut c_void);
    pub fn ghostty_renderer_resize(renderer: GhosttyRenderer, width: u32, height: u32);
    pub fn ghostty_renderer_update_frame(renderer: GhosttyRenderer, cursor_blink_visible: bool);
    pub fn ghostty_renderer_bg_cells(renderer: GhosttyRenderer, count: *mut u32) -> *const [u8; 4];
    pub fn ghostty_renderer_text_cells(
        renderer: GhosttyRenderer,
        count: *mut u32,
    ) -> *const GhosttyRendererCellText;
    pub fn ghostty_renderer_frame_data(
        renderer: GhosttyRenderer,
        out: *mut GhosttyRendererFrameData,
    );

    // Theme & config
    pub fn ghostty_renderer_load_theme(renderer: GhosttyRenderer, name: *const i8) -> bool;
    pub fn ghostty_renderer_load_theme_file(renderer: GhosttyRenderer, path: *const i8) -> bool;
    pub fn ghostty_renderer_set_background(renderer: GhosttyRenderer, r: u8, g: u8, b: u8);
    pub fn ghostty_renderer_set_foreground(renderer: GhosttyRenderer, r: u8, g: u8, b: u8);
    pub fn ghostty_renderer_set_background_opacity(renderer: GhosttyRenderer, opacity: f32);
    pub fn ghostty_renderer_set_min_contrast(renderer: GhosttyRenderer, contrast: f32);
    pub fn ghostty_renderer_set_palette(
        renderer: GhosttyRenderer,
        palette: *const GhosttyRendererRGB,
    );
}
