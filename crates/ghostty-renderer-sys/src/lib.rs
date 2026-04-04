#![allow(non_camel_case_types)]

use std::ffi::c_void;

pub type GhosttyRenderer = *mut c_void;
pub type GhosttyTerminal = *mut c_void;

#[repr(C)]
pub struct GhosttyRendererConfig {
    pub nsview: *mut c_void,
    pub content_scale: f64,
    pub width: u32,
    pub height: u32,
    pub font_size: f32,
}

unsafe extern "C" {
    pub fn ghostty_renderer_new(config: *const GhosttyRendererConfig) -> GhosttyRenderer;
    pub fn ghostty_renderer_free(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_set_terminal(renderer: GhosttyRenderer, terminal: GhosttyTerminal);
    pub fn ghostty_renderer_update_frame(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_draw_frame(renderer: GhosttyRenderer);
    pub fn ghostty_renderer_resize(renderer: GhosttyRenderer, width: u32, height: u32, scale: f64);

    pub fn ghostty_terminal_new(cols: u16, rows: u16, max_scrollback: u32) -> GhosttyTerminal;
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);
}
