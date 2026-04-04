#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type GhosttyRenderer = *mut c_void;
pub type GhosttyTerminal = *mut c_void;

#[repr(C)]
pub struct GhosttyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
pub struct GhosttyOptColor {
    pub color: GhosttyColor,
    pub has: bool,
}

#[repr(C)]
#[derive(Default)]
pub struct GhosttyRendererConfig {
    pub width_px: u32,
    pub height_px: u32,
    pub content_scale: f64,

    pub font_size: f32,
    pub font_family: *const i8,
    pub font_family_bold: *const i8,
    pub font_family_italic: *const i8,
    pub font_family_bold_italic: *const i8,
    pub font_features: *const i8,
    pub font_thicken: bool,
    pub font_thicken_strength: u8,

    pub background: GhosttyColor,
    pub foreground: GhosttyColor,
    pub cursor_color: GhosttyOptColor,
    pub cursor_text: GhosttyOptColor,
    pub selection_background: GhosttyOptColor,
    pub selection_foreground: GhosttyOptColor,
    pub bold_color: GhosttyOptColor,

    pub search_match_bg: GhosttyColor,
    pub search_match_fg: GhosttyColor,
    pub search_selected_bg: GhosttyColor,
    pub search_selected_fg: GhosttyColor,

    pub background_opacity: f32,
    pub cursor_opacity: f32,
    pub faint_opacity: f32,

    pub min_contrast: f32,
    pub colorspace: i32,
    pub alpha_blending: i32,
    pub padding_color: i32,

    pub scroll_to_bottom_on_output: bool,
}

#[repr(C)]
pub struct GhosttyRendererCellBg {
    pub rgba: [u8; 4],
}

#[repr(C)]
pub struct GhosttyRendererCellText {
    pub glyph_pos: [u32; 2],
    pub glyph_size: [u32; 2],
    pub bearings: [i16; 2],
    pub grid_pos: [u16; 2],
    pub color: [u8; 4],
    pub atlas: u8,
    pub flags: u8,
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

unsafe extern "C" {
    // Renderer lifecycle
    pub fn ghostty_renderer_new(config: *const GhosttyRendererConfig, native_handle: *mut c_void) -> GhosttyRenderer;
    pub fn ghostty_renderer_free(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_set_terminal(renderer: GhosttyRenderer, terminal: GhosttyTerminal);
    pub fn ghostty_renderer_resize(renderer: GhosttyRenderer, width: u32, height: u32, scale: f64);

    // Theme
    pub fn ghostty_renderer_load_theme(renderer: GhosttyRenderer, name: *const i8) -> bool;
    pub fn ghostty_renderer_load_theme_file(renderer: GhosttyRenderer, path: *const i8) -> bool;

    // Runtime config
    pub fn ghostty_renderer_set_font_size(renderer: GhosttyRenderer, points: f32);
    pub fn ghostty_renderer_set_background(renderer: GhosttyRenderer, r: u8, g: u8, b: u8);
    pub fn ghostty_renderer_set_foreground(renderer: GhosttyRenderer, r: u8, g: u8, b: u8);
    pub fn ghostty_renderer_set_background_opacity(renderer: GhosttyRenderer, opacity: f32);
    pub fn ghostty_renderer_set_min_contrast(renderer: GhosttyRenderer, contrast: f32);

    // Level 1
    pub fn ghostty_renderer_update_frame(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_draw_frame(renderer: GhosttyRenderer);

    // Level 2
    pub fn ghostty_renderer_get_bg_cells(renderer: GhosttyRenderer, count: *mut u32) -> *const [u8; 4];
    pub fn ghostty_renderer_get_text_cells(renderer: GhosttyRenderer, count: *mut u32) -> *const GhosttyRendererCellText;
    pub fn ghostty_renderer_get_frame_data(renderer: GhosttyRenderer, out: *mut GhosttyRendererFrameData);
    pub fn ghostty_renderer_get_atlas_grayscale(renderer: GhosttyRenderer, size: *mut u32, modified: *mut bool) -> *const u8;
    pub fn ghostty_renderer_get_atlas_color(renderer: GhosttyRenderer, size: *mut u32, modified: *mut bool) -> *const u8;

    // Terminal
    pub fn ghostty_terminal_new(cols: u16, rows: u16, max_scrollback: u32) -> GhosttyTerminal;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
    pub fn ghostty_terminal_resize(terminal: GhosttyTerminal, cols: u16, rows: u16);
    pub fn ghostty_terminal_drain_responses(terminal: GhosttyTerminal, out_len: *mut usize) -> *const u8;
    pub fn ghostty_terminal_clear_responses(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_get_size(terminal: GhosttyTerminal, cols: *mut u16, rows: *mut u16);
    pub fn ghostty_terminal_get_cursor(terminal: GhosttyTerminal, col: *mut u16, row: *mut u16, visible: *mut bool);
    pub fn ghostty_terminal_scroll(terminal: GhosttyTerminal, action: i32, delta: i32);
    pub fn ghostty_terminal_get_scrollback_rows(terminal: GhosttyTerminal) -> usize;
    pub fn ghostty_free(ptr: *mut c_void);
}

impl Default for GhosttyColor {
    fn default() -> Self {
        Self { r: 0, g: 0, b: 0 }
    }
}

impl Default for GhosttyOptColor {
    fn default() -> Self {
        Self {
            color: GhosttyColor::default(),
            has: false,
        }
    }
}
