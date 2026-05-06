//! Per-surface state: everything that exists only while an OS window is up.
//!
//! Created in `App::resumed`, torn down when the pane exits. Bundling these
//! fields keeps `App` focused on process-lifetime state (config, input handler,
//! config watcher). The name `SurfaceState` avoids reserving `Window` for the
//! future mux domain (`Window -> Tab -> SplitTree`).

use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use winit::dpi::PhysicalSize;
use winit::event::Modifiers;
use winit::window::Window;

use seance_mux::{
    CursorShape as MuxCursorShape, GridPos, Pane, Resize, TerminalModes, ThemeColors,
};
use seance_render::{RenderInputs, TerminalRenderer};

use crate::mouse::MouseState;

pub(crate) struct SurfaceState {
    pub(crate) window: Arc<Window>,
    pub(crate) renderer: TerminalRenderer,
    pub(crate) pane: Pane,
    pub(crate) render_inputs: RenderInputs,
    pub(crate) modifiers: Modifiers,
    pub(crate) cell_size: [f32; 2],
    pub(crate) content_dirty: bool,
    pub(crate) occluded: bool,
    pub(crate) mouse: MouseState,
    pub(crate) blink_on: bool,
    pub(crate) last_blink_edge: Instant,
    pub(crate) last_vt_cursor_shape: Option<MuxCursorShape>,
}

impl SurfaceState {
    pub(crate) fn new(
        window: Arc<Window>,
        renderer: TerminalRenderer,
        pane: Pane,
        render_inputs: RenderInputs,
    ) -> Self {
        let cell_size = renderer.cell_size();
        Self {
            window,
            renderer,
            pane,
            render_inputs,
            modifiers: Modifiers::default(),
            cell_size,
            content_dirty: true,
            occluded: false,
            mouse: MouseState::default(),
            blink_on: true,
            last_blink_edge: Instant::now(),
            last_vt_cursor_shape: None,
        }
    }

    pub(crate) fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.content_dirty = true;
        self.request_redraw();
    }

    /// Resize the surface and ask the local pane to reflow the terminal grid.
    ///
    /// The redraw is intentionally deferred until the pane publishes the
    /// resized frame, avoiding a frame rendered against stale grid data.
    pub(crate) fn reflow(&mut self, pixel_size: PhysicalSize<u32>) {
        self.renderer
            .resize_surface(pixel_size.width, pixel_size.height);
        self.cell_size = self.renderer.cell_size();
        let (cols, rows) = self.renderer.grid_size();
        if let Err(err) = self.pane.resize(Resize {
            cols,
            rows,
            pixel_width: pixel_size.width as u16,
            pixel_height: pixel_size.height as u16,
        }) {
            log::warn!("failed to send pane resize: {err}");
        }
    }

    pub(crate) fn terminal_modes(&self) -> TerminalModes {
        self.pane.modes()
    }

    pub(crate) fn write_to_pty(&self, bytes: Bytes) {
        if let Err(err) = self.pane.write(bytes) {
            log::warn!("failed to send pane write: {err}");
        }
    }

    pub(crate) fn scroll_lines(&self, delta: i32) {
        if let Err(err) = self.pane.scroll_lines(delta) {
            log::warn!("failed to send pane scroll: {err}");
        }
    }

    pub(crate) fn set_theme_colors(&self, theme: &seance_config::Theme) {
        if let Err(err) = self.pane.set_theme_colors(ThemeColors {
            fg: theme.fg,
            bg: [theme.bg[0], theme.bg[1], theme.bg[2]],
            cursor: [theme.cursor[0], theme.cursor[1], theme.cursor[2]],
            palette: theme.palette,
        }) {
            log::warn!("failed to send pane theme colors: {err}");
        }
    }

    pub(crate) fn set_cursor_shape(&self, shape: MuxCursorShape) {
        if let Err(err) = self.pane.set_cursor_shape(shape) {
            log::warn!("failed to send pane cursor shape: {err}");
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.pane.has_selection()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.pane.clear_selection();
        self.render_inputs.selection = None;
    }

    pub(crate) fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.pane.selection_range()
    }

    pub(crate) fn sync_selection_to_overlay(&mut self) {
        self.render_inputs.selection = self.selection_range();
    }

    pub(crate) fn start_selection(&mut self, col: u16, row: u16) {
        self.pane.start_selection(col, row);
    }

    pub(crate) fn start_word_selection(&mut self, col: u16, row: u16) {
        self.pane.start_word_selection(col, row);
    }

    pub(crate) fn start_line_selection(&mut self, row: u16) {
        self.pane.start_line_selection(row);
    }

    pub(crate) fn update_selection(&mut self, col: u16, row: u16) {
        self.pane.update_selection(col, row);
    }

    pub(crate) fn select_all(&mut self) {
        let (cols, rows) = self.renderer.grid_size();
        self.pane.select_all(cols, rows);
    }

    pub(crate) fn copy_selection_to_clipboard(&self) {
        let Some(text) = self.pane.selection_text() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(text);
        }
    }

    pub(crate) fn paste_from_clipboard(&self) {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return;
        };
        let Ok(text) = cb.get_text() else {
            return;
        };
        let bracketed = self.terminal_modes().bracketed_paste;
        let mut bytes = BytesMut::with_capacity(text.len() + if bracketed { 12 } else { 0 });
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.write_to_pty(bytes.freeze());
    }
}
