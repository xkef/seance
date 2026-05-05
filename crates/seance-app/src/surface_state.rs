//! Per-surface state: everything that exists only while an OS window is up.
//!
//! Created in `App::resumed`, torn down when the pane's VT session exits.
//! Bundling these fields keeps `App` focused on process-lifetime state
//! (config, input handler, config watcher). The name `SurfaceState` avoids
//! reserving `Window` for the future mux domain (`Window -> Tab -> SplitTree`).

use std::sync::Arc;
use std::time::Instant;

use bytes::{Bytes, BytesMut};
use winit::dpi::PhysicalSize;
use winit::event::Modifiers;
use winit::window::Window;

use seance_render::{RenderInputs, TerminalRenderer};
use seance_vt::{
    CursorShape as VtCursorShape, GridPos, Resize, Selection, TerminalModes, ThemeColors,
    VtSessionHandle, VtSnapshot,
};

use crate::mouse::MouseState;

pub(crate) struct PaneSession {
    pub(crate) vt: VtSessionHandle,
    pub(crate) latest_snapshot: Option<Arc<VtSnapshot>>,
    pub(crate) selection: Option<Selection>,
}

impl PaneSession {
    pub(crate) fn new(vt: VtSessionHandle) -> Self {
        let latest_snapshot = vt.latest_snapshot();
        Self {
            vt,
            latest_snapshot,
            selection: None,
        }
    }

    pub(crate) fn refresh_latest_snapshot(&mut self) {
        self.vt.clear_content_dirty_pending();
        if let Some(snapshot) = self.vt.latest_snapshot() {
            self.latest_snapshot = Some(snapshot);
        }
    }

    pub(crate) fn ack_rendered(&self, generation: u64) {
        if let Err(err) = self.vt.ack_rendered(generation) {
            log::warn!("failed to ack rendered VT snapshot: {err}");
        }
    }

    fn modes(&self) -> TerminalModes {
        self.latest_snapshot
            .as_ref()
            .map_or(TerminalModes::default(), |snapshot| snapshot.modes)
    }
}

pub(crate) struct SurfaceState {
    pub(crate) window: Arc<Window>,
    pub(crate) renderer: TerminalRenderer,
    pub(crate) pane: PaneSession,
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
    // opinion (e.g. snapshot extraction error path).
    pub(crate) last_vt_cursor_shape: Option<VtCursorShape>,
}

impl SurfaceState {
    pub(crate) fn new(
        window: Arc<Window>,
        renderer: TerminalRenderer,
        vt: VtSessionHandle,
        render_inputs: RenderInputs,
    ) -> Self {
        let cell_size = renderer.cell_size();
        Self {
            window,
            renderer,
            pane: PaneSession::new(vt),
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

    /// Resize the surface and ask the IO actor to reflow the VT grid.
    ///
    /// The redraw is intentionally deferred until the actor publishes the
    /// resized snapshot, avoiding a frame rendered against stale grid data.
    pub(crate) fn reflow(&mut self, pixel_size: PhysicalSize<u32>) {
        self.renderer
            .resize_surface(pixel_size.width, pixel_size.height);
        self.cell_size = self.renderer.cell_size();
        let (cols, rows) = self.renderer.grid_size();
        if let Err(err) = self.pane.vt.resize(Resize {
            cols,
            rows,
            pixel_width: pixel_size.width as u16,
            pixel_height: pixel_size.height as u16,
        }) {
            log::warn!("failed to send VT resize: {err}");
        }
    }

    pub(crate) fn terminal_modes(&self) -> TerminalModes {
        self.pane.modes()
    }

    pub(crate) fn write_to_pty(&self, bytes: Bytes) {
        if let Err(err) = self.pane.vt.write(bytes) {
            log::warn!("failed to send VT write: {err}");
        }
    }

    pub(crate) fn scroll_lines(&self, delta: i32) {
        if let Err(err) = self.pane.vt.scroll_lines(delta) {
            log::warn!("failed to send VT scroll: {err}");
        }
    }

    pub(crate) fn set_theme_colors(&self, theme: &seance_config::Theme) {
        if let Err(err) = self.pane.vt.set_theme_colors(ThemeColors {
            fg: theme.fg,
            bg: [theme.bg[0], theme.bg[1], theme.bg[2]],
            cursor: [theme.cursor[0], theme.cursor[1], theme.cursor[2]],
            palette: theme.palette,
        }) {
            log::warn!("failed to send VT theme colors: {err}");
        }
    }

    pub(crate) fn set_cursor_shape(&self, shape: VtCursorShape) {
        if let Err(err) = self.pane.vt.set_cursor_shape(shape) {
            log::warn!("failed to send VT cursor shape: {err}");
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.pane.selection.is_some()
    }

    pub(crate) fn clear_selection(&mut self) {
        self.pane.selection = None;
        self.render_inputs.selection = None;
    }

    pub(crate) fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.pane.selection.as_ref().map(Selection::ordered_range)
    }

    pub(crate) fn sync_selection_to_overlay(&mut self) {
        self.render_inputs.selection = self.selection_range();
    }

    pub(crate) fn start_selection(&mut self, col: u16, row: u16) {
        self.pane.selection = Some(Selection::new(GridPos { col, row }));
    }

    pub(crate) fn start_word_selection(&mut self, col: u16, row: u16) {
        self.pane.selection = Some(Selection::new_word(GridPos { col, row }));
    }

    pub(crate) fn start_line_selection(&mut self, row: u16) {
        self.pane.selection = Some(Selection::new_line(GridPos { col: 0, row }));
    }

    pub(crate) fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(selection) = &mut self.pane.selection {
            selection.update(GridPos { col, row });
        }
    }

    pub(crate) fn select_all(&mut self) {
        let (cols, rows) = self.renderer.grid_size();
        let mut selection = Selection::new_line(GridPos { col: 0, row: 0 });
        selection.update(GridPos {
            col: cols.saturating_sub(1),
            row: rows.saturating_sub(1),
        });
        self.pane.selection = Some(selection);
    }

    pub(crate) fn copy_selection_to_clipboard(&self) {
        let Some(selection) = self.pane.selection.as_ref() else {
            return;
        };
        let Some(snapshot) = self.pane.latest_snapshot.as_ref() else {
            return;
        };
        let Some(text) = snapshot.selection_text(selection) else {
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
