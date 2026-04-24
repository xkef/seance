//! Per-window state: everything that exists only while a window is up.
//!
//! Created in `App::resumed`, torn down when the terminal dies. Bundling
//! these fields keeps `App` focused on process-lifetime state (config,
//! input handler, config watcher).

use std::sync::Arc;
use std::time::Instant;

use winit::dpi::PhysicalSize;
use winit::event::Modifiers;
use winit::window::Window;

use seance_render::{RenderInputs, TerminalRenderer};
use seance_vt::{CursorShape as VtCursorShape, Terminal, TerminalModes};

use crate::mouse::MouseState;

pub(crate) struct WindowState {
    pub(crate) window: Arc<Window>,
    pub(crate) renderer: TerminalRenderer,
    pub(crate) terminal: Terminal,
    pub(crate) render_inputs: RenderInputs,
    pub(crate) modifiers: Modifiers,
    pub(crate) cell_size: [f32; 2],
    pub(crate) content_dirty: bool,
    pub(crate) occluded: bool,
    pub(crate) mouse: MouseState,
    pub(crate) blink_on: bool,
    pub(crate) last_blink_edge: Instant,
    // `None` until the VT has reported a shape via DECSCUSR; then the
    // config's `cursor.style` acts as the fallback when the VT has no
    // opinion (e.g. FFI error path in `LibGhosttyFrameSource::cursor`).
    pub(crate) last_vt_cursor_shape: Option<VtCursorShape>,
}

impl WindowState {
    pub(crate) fn new(
        window: Arc<Window>,
        renderer: TerminalRenderer,
        terminal: Terminal,
        render_inputs: RenderInputs,
    ) -> Self {
        let cell_size = renderer.cell_size();
        Self {
            window,
            renderer,
            terminal,
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

    /// Read pending PTY output. Returns `false` when the child has exited —
    /// caller should tear the window down.
    pub(crate) fn poll_pty(&mut self) -> bool {
        if self.terminal.poll() {
            self.content_dirty = true;
        }
        self.terminal.is_alive()
    }

    /// Resize the surface and reflow the VT grid.
    pub(crate) fn reflow(&mut self, pixel_size: PhysicalSize<u32>) {
        self.renderer
            .resize_surface(pixel_size.width, pixel_size.height);
        self.cell_size = self.renderer.cell_size();
        let (cols, rows) = self.renderer.grid_size();
        self.terminal.resize(
            cols,
            rows,
            pixel_size.width as u16,
            pixel_size.height as u16,
        );
        self.mark_dirty();
    }

    pub(crate) fn terminal_modes(&self) -> TerminalModes {
        self.terminal.modes()
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.terminal.has_selection()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.terminal.clear_selection();
        self.render_inputs.selection = None;
    }

    pub(crate) fn sync_selection_to_overlay(&mut self) {
        self.render_inputs.selection = self.terminal.selection_range();
    }

    pub(crate) fn copy_selection_to_clipboard(&mut self) {
        let Some(text) = self.terminal.selection_text() else {
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
        let bracketed = self.terminal.modes().bracketed_paste;
        if bracketed {
            self.terminal.write(b"\x1b[200~");
        }
        self.terminal.write(text.as_bytes());
        if bracketed {
            self.terminal.write(b"\x1b[201~");
        }
    }
}
