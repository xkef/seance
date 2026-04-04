use std::ffi::c_void;

use ghostty_renderer_sys as ffi;

pub struct RendererConfig {
    pub nsview: *mut c_void,
    pub content_scale: f64,
    pub width: u32,
    pub height: u32,
    pub font_size: f32,
}

pub struct Renderer {
    raw: ffi::GhosttyRenderer,
}

impl Renderer {
    pub fn new(config: &RendererConfig) -> Option<Self> {
        let c = ffi::GhosttyRendererConfig {
            nsview: config.nsview,
            content_scale: config.content_scale,
            width: config.width,
            height: config.height,
            font_size: config.font_size,
        };
        let raw = unsafe { ffi::ghostty_renderer_new(&c) };
        if raw.is_null() { None } else { Some(Self { raw }) }
    }

    pub fn set_terminal(&self, terminal: &Terminal) {
        unsafe { ffi::ghostty_renderer_set_terminal(self.raw, terminal.raw) };
    }

    pub fn update_frame(&self) {
        unsafe { ffi::ghostty_renderer_update_frame(self.raw) };
    }

    pub fn draw_frame(&self) {
        unsafe { ffi::ghostty_renderer_draw_frame(self.raw) };
    }

    pub fn resize(&self, width: u32, height: u32, scale: f64) {
        unsafe { ffi::ghostty_renderer_resize(self.raw, width, height, scale) };
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_renderer_free(self.raw) };
    }
}

pub struct Terminal {
    raw: ffi::GhosttyTerminal,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16) -> Option<Self> {
        let raw = unsafe { ffi::ghostty_terminal_new(cols, rows, 10_000) };
        if raw.is_null() { None } else { Some(Self { raw }) }
    }

    pub fn vt_write(&self, data: &[u8]) {
        unsafe { ffi::ghostty_terminal_vt_write(self.raw, data.as_ptr(), data.len()) };
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_terminal_free(self.raw) };
    }
}
