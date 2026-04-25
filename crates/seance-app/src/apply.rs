//! `apply_*` methods: propagate a settings change (font size, scale factor,
//! window padding) into the renderer and reflow the PTY. Split from `app.rs`
//! because they form a tight thematic cluster.

use crate::app::{App, physical_window_padding};

impl App {
    pub(crate) fn apply_font_size(&mut self) {
        let font_size = self.font_size;
        if let Some(ws) = self.ws_mut() {
            ws.renderer.set_font_size(font_size);
            ws.reflow(ws.window.inner_size());
        }
    }

    pub(crate) fn apply_font_metrics(
        &mut self,
        font_size_changed: bool,
        adjust_cell_height_changed: bool,
    ) {
        let font_size = self.font_size;
        let adjust = self.config.font.adjust_cell_height.clone();
        if let Some(ws) = self.ws_mut() {
            if adjust_cell_height_changed {
                ws.renderer.set_adjust_cell_height(adjust.as_deref());
            }
            if font_size_changed {
                ws.renderer.set_font_size(font_size);
            }
            ws.reflow(ws.window.inner_size());
        }
    }

    pub(crate) fn apply_scale_factor(&mut self, scale_factor: f64) {
        let padding = physical_window_padding(&self.config, scale_factor);
        if let Some(ws) = self.ws_mut() {
            ws.renderer.set_scale(scale_factor);
            ws.renderer.set_window_padding(padding);
            ws.reflow(ws.window.inner_size());
        }
    }

    /// Push the configured window padding to the renderer and reflow the PTY.
    /// `grid_size()` shrinks when padding grows, so a reflow is required to
    /// keep the shell's SIGWINCH in sync.
    pub(crate) fn apply_window_padding(&mut self) {
        let config = &self.config;
        if let Some(ws) = self.window_state.as_mut() {
            let padding = physical_window_padding(config, ws.window.scale_factor());
            ws.renderer.set_window_padding(padding);
            ws.reflow(ws.window.inner_size());
        }
    }
}
