//! Raw FFI bindings to libghostty-renderer.
//!
//! These mirror the C header `include/ghostty/renderer.h` exactly.
//! No safety wrappers — see the `ghostty-renderer` crate for those.

#![allow(non_camel_case_types)]

use std::ffi::c_void;

// ================================================================
// Handles
// ================================================================

pub type GhosttyRenderer = *mut c_void;
pub type GhosttyTerminal = *mut c_void;

// ================================================================
// Basic types
// ================================================================

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct GhosttyOptColor {
    pub color: GhosttyColor,
    pub has: bool,
}

// ================================================================
// Enums
// ================================================================

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
// Renderer configuration — zero-init produces usable defaults
// ================================================================

#[repr(C)]
#[derive(Default)]
pub struct GhosttyRendererConfig {
    // Surface
    pub width_px: u32,
    pub height_px: u32,
    pub content_scale: f64,

    // Font
    pub font_size: f32,
    pub font_family: *const i8,
    pub font_family_bold: *const i8,
    pub font_family_italic: *const i8,
    pub font_family_bold_italic: *const i8,
    pub font_features: *const i8,
    pub font_thicken: bool,
    pub font_thicken_strength: u8,

    // Colors
    pub background: GhosttyColor,
    pub foreground: GhosttyColor,
    pub cursor_color: GhosttyOptColor,
    pub cursor_text: GhosttyOptColor,
    pub selection_background: GhosttyOptColor,
    pub selection_foreground: GhosttyOptColor,
    pub bold_color: GhosttyOptColor,

    // Search colors
    pub search_match_bg: GhosttyColor,
    pub search_match_fg: GhosttyColor,
    pub search_selected_bg: GhosttyColor,
    pub search_selected_fg: GhosttyColor,

    // Opacity
    pub background_opacity: f32,
    pub cursor_opacity: f32,
    pub faint_opacity: f32,

    // Rendering
    pub min_contrast: f32,
    pub colorspace: GhosttyColorspace,
    pub alpha_blending: GhosttyBlending,
    pub padding_color: GhosttyPaddingColor,

    // Behavior
    pub scroll_to_bottom_on_output: bool,
}

// ================================================================
// Level 2 data types
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
    /// Padding in pixels: [left, top, right, bottom].
    pub grid_padding: [f32; 4],
    pub bg_color: [u8; 4],
    pub min_contrast: f32,
    pub cursor_pos: [u16; 2],
    pub cursor_color: [u8; 4],
    pub cursor_wide: bool,
}

// ================================================================
// Scroll action enum
// ================================================================

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub enum GhosttyScrollAction {
    Lines = 0,
    Top = 1,
    Bottom = 2,
    PageUp = 3,
    PageDown = 4,
}

// ================================================================
// C API functions
// ================================================================

unsafe extern "C" {
    // Renderer lifecycle
    pub fn ghostty_renderer_new(
        config: *const GhosttyRendererConfig,
        native_handle: *mut c_void,
    ) -> GhosttyRenderer;
    pub fn ghostty_renderer_free(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_set_terminal(renderer: GhosttyRenderer, terminal: GhosttyTerminal);
    pub fn ghostty_renderer_resize(
        renderer: GhosttyRenderer,
        width: u32,
        height: u32,
        scale: f64,
    );

    // Theme
    pub fn ghostty_renderer_load_theme(renderer: GhosttyRenderer, name: *const i8) -> bool;
    pub fn ghostty_renderer_load_theme_file(renderer: GhosttyRenderer, path: *const i8) -> bool;

    // Runtime config
    pub fn ghostty_renderer_set_font_size(renderer: GhosttyRenderer, points: f32);
    pub fn ghostty_renderer_set_background(renderer: GhosttyRenderer, c: GhosttyColor);
    pub fn ghostty_renderer_set_foreground(renderer: GhosttyRenderer, c: GhosttyColor);
    pub fn ghostty_renderer_set_background_opacity(renderer: GhosttyRenderer, opacity: f32);
    pub fn ghostty_renderer_set_min_contrast(renderer: GhosttyRenderer, contrast: f32);
    pub fn ghostty_renderer_set_palette(renderer: GhosttyRenderer, palette: *const GhosttyColor);

    // Level 1
    pub fn ghostty_renderer_update_frame(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_draw_frame(renderer: GhosttyRenderer);

    // Level 2
    pub fn ghostty_renderer_get_bg_cells(
        renderer: GhosttyRenderer,
        count: *mut u32,
    ) -> *const [u8; 4];
    pub fn ghostty_renderer_get_text_cells(
        renderer: GhosttyRenderer,
        count: *mut u32,
    ) -> *const GhosttyRendererCellText;
    pub fn ghostty_renderer_get_frame_data(
        renderer: GhosttyRenderer,
        out: *mut GhosttyRendererFrameData,
    );
    pub fn ghostty_renderer_get_atlas_grayscale(
        renderer: GhosttyRenderer,
        size: *mut u32,
        modified: *mut bool,
    ) -> *const u8;
    pub fn ghostty_renderer_get_atlas_color(
        renderer: GhosttyRenderer,
        size: *mut u32,
        modified: *mut bool,
    ) -> *const u8;

    // Terminal lifecycle
    pub fn ghostty_terminal_new(
        cols: u16,
        rows: u16,
        max_scrollback: u32,
    ) -> GhosttyTerminal;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_vt_write(
        terminal: GhosttyTerminal,
        data: *const u8,
        len: usize,
    );
    pub fn ghostty_terminal_resize(terminal: GhosttyTerminal, cols: u16, rows: u16);
    pub fn ghostty_terminal_drain_responses(
        terminal: GhosttyTerminal,
        out_len: *mut usize,
    ) -> *const u8;
    pub fn ghostty_terminal_clear_responses(terminal: GhosttyTerminal);

    // Terminal state queries
    pub fn ghostty_terminal_get_size(
        terminal: GhosttyTerminal,
        cols: *mut u16,
        rows: *mut u16,
    );
    pub fn ghostty_terminal_get_cursor(
        terminal: GhosttyTerminal,
        col: *mut u16,
        row: *mut u16,
        visible: *mut bool,
    );
    pub fn ghostty_terminal_get_title(
        terminal: GhosttyTerminal,
        len: *mut usize,
    ) -> *const u8;

    // Scrolling
    pub fn ghostty_terminal_scroll(
        terminal: GhosttyTerminal,
        action: GhosttyScrollAction,
        delta: i32,
    );
    pub fn ghostty_terminal_get_scrollback_rows(terminal: GhosttyTerminal) -> usize;

    // Terminal modes
    pub fn ghostty_terminal_mode_cursor_keys(terminal: GhosttyTerminal) -> bool;
    pub fn ghostty_terminal_mode_mouse_event(terminal: GhosttyTerminal) -> i32;
    pub fn ghostty_terminal_mode_mouse_format_sgr(terminal: GhosttyTerminal) -> bool;
    pub fn ghostty_terminal_mode_synchronized_output(terminal: GhosttyTerminal) -> bool;
    pub fn ghostty_terminal_dump_screen(
        terminal: GhosttyTerminal,
        buf: *mut u8,
        buf_len: usize,
    ) -> usize;

    // Memory
    pub fn ghostty_free(ptr: *mut c_void);
}
