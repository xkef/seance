//! Per-surface state: everything that exists only while an OS window is up.
//!
//! Created in `App::resumed`, torn down when the pane exits. Bundling these
//! fields keeps `App` focused on process-lifetime state (config, input handler,
//! config watcher). The name `SurfaceState` avoids reserving `Window` for the
//! future mux domain (`Window -> Tab -> SplitTree`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use winit::dpi::PhysicalSize;
use winit::event::Modifiers;
use winit::window::{CursorIcon, Window};

use seance_mux::{
    ClientRefresh, CursorShape as MuxCursorShape, DetectedLink, GridPos, LinkModifiers, LinkTarget,
    LocalDomain, MuxClient, PaneRef, Resize, TerminalModes, ThemeColors,
};
use seance_render::{HoveredLinkRange, RenderInputs, TerminalRenderer};

use crate::mouse::MouseState;

pub(crate) struct SurfaceState {
    pub(crate) window: Arc<Window>,
    pub(crate) renderer: TerminalRenderer,
    pub(crate) mux: MuxClient<LocalDomain>,
    pub(crate) active_pane: PaneRef,
    pub(crate) render_inputs: RenderInputs,
    pub(crate) modifiers: Modifiers,
    pub(crate) cell_size: [f32; 2],
    pub(crate) content_dirty: bool,
    pub(crate) occluded: bool,
    pub(crate) mouse: MouseState,
    pub(crate) blink_on: bool,
    pub(crate) last_blink_edge: Instant,
    pub(crate) last_vt_cursor_shape: Option<MuxCursorShape>,
    /// When set, the active selection should be cleared once `Instant::now()`
    /// reaches this deadline. Drives the brief "flash" overlay shown after
    /// double/triple-click and Enter-copies operations.
    pub(crate) selection_dismiss_at: Option<Instant>,
}

impl SurfaceState {
    pub(crate) fn new(
        window: Arc<Window>,
        renderer: TerminalRenderer,
        mux: MuxClient<LocalDomain>,
        active_pane: PaneRef,
        render_inputs: RenderInputs,
    ) -> Self {
        let cell_size = renderer.cell_size();
        Self {
            window,
            renderer,
            mux,
            active_pane,
            render_inputs,
            modifiers: Modifiers::default(),
            cell_size,
            content_dirty: true,
            occluded: false,
            mouse: MouseState::default(),
            blink_on: true,
            last_blink_edge: Instant::now(),
            last_vt_cursor_shape: None,
            selection_dismiss_at: None,
        }
    }

    pub(crate) fn request_redraw(&self) {
        self.window.request_redraw();
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.content_dirty = true;
        self.request_redraw();
    }

    pub(crate) fn refresh_updates(&mut self) -> Result<ClientRefresh, seance_mux::PaneError> {
        self.mux.refresh_updates()
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
        if let Err(err) = self.mux.pane(self.active_pane).resize(Resize {
            cols,
            rows,
            pixel_width: pixel_size.width as u16,
            pixel_height: pixel_size.height as u16,
        }) {
            // Send errors here mean the VT actor's command channel is
            // closed — expected during pane teardown, not a recoverable
            // failure. Same applies to every other `mux.pane(...).<send>`
            // call in this file.
            tracing::debug!("failed to send pane resize: {err}");
        }
    }

    pub(crate) fn terminal_modes(&self) -> TerminalModes {
        self.mux
            .pane_view(self.active_pane)
            .map_or(TerminalModes::default(), |view| view.modes())
    }

    pub(crate) fn write_to_pty(&mut self, bytes: Bytes) {
        if let Err(err) = self.mux.pane(self.active_pane).write(bytes) {
            tracing::debug!("failed to send pane write: {err}");
        }
    }

    pub(crate) fn scroll_lines(&mut self, delta: i32) {
        if let Err(err) = self.mux.pane(self.active_pane).scroll_lines(delta) {
            tracing::debug!("failed to send pane scroll: {err}");
        }
    }

    pub(crate) fn set_theme_colors(&mut self, theme: &seance_config::Theme) {
        if let Err(err) = self
            .mux
            .pane(self.active_pane)
            .set_theme_colors(ThemeColors {
                fg: theme.fg,
                bg: [theme.bg[0], theme.bg[1], theme.bg[2]],
                cursor: [theme.cursor[0], theme.cursor[1], theme.cursor[2]],
                palette: theme.palette,
            })
        {
            tracing::debug!("failed to send pane theme colors: {err}");
        }
    }

    pub(crate) fn set_cursor_shape(&mut self, shape: MuxCursorShape) {
        if let Err(err) = self.mux.pane(self.active_pane).set_cursor_shape(shape) {
            tracing::debug!("failed to send pane cursor shape: {err}");
        }
    }

    pub(crate) fn ack_presented(&mut self, generation: u64) {
        if let Err(err) = self.mux.pane(self.active_pane).ack_presented(generation) {
            tracing::debug!("failed to ack presented pane frame: {err}");
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.mux
            .pane_view(self.active_pane)
            .is_some_and(|view| view.has_selection())
    }

    pub(crate) fn clear_selection(&mut self) {
        self.mux.pane(self.active_pane).clear_selection();
    }

    pub(crate) fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.mux
            .pane_view(self.active_pane)
            .and_then(|view| view.selection_range())
    }

    pub(crate) fn start_selection(&mut self, col: u16, row: u16) {
        self.mux.pane(self.active_pane).start_selection(col, row);
    }

    pub(crate) fn start_word_selection(&mut self, col: u16, row: u16) {
        self.mux
            .pane(self.active_pane)
            .start_word_selection(col, row);
    }

    pub(crate) fn start_line_selection(&mut self, row: u16) {
        self.mux.pane(self.active_pane).start_line_selection(row);
    }

    pub(crate) fn update_selection(&mut self, col: u16, row: u16) {
        self.mux.pane(self.active_pane).update_selection(col, row);
    }

    pub(crate) fn select_all(&mut self) {
        let (cols, rows) = self.renderer.grid_size();
        self.mux.pane(self.active_pane).select_all(cols, rows);
    }

    pub(crate) fn copy_selection_to_clipboard(&self) {
        let Some(text) = self
            .mux
            .pane_view(self.active_pane)
            .and_then(|view| view.selection_text())
        else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(text);
        }
    }

    /// Copy the current selection to the clipboard and arm a deferred
    /// dismiss `delay` from now. The selection stays painted during the
    /// flash window so the user gets a visible confirmation that the
    /// copy happened, then `App` clears it on the next animation tick.
    pub(crate) fn flash_copy_selection(&mut self, delay: Duration) {
        if !self.has_selection() {
            return;
        }
        self.copy_selection_to_clipboard();
        self.selection_dismiss_at = Some(Instant::now() + delay);
        self.mark_dirty();
    }

    /// Clear the deferred-dismiss state without touching the selection.
    /// Called when the user starts a new selection during the flash.
    pub(crate) fn cancel_flash_dismiss(&mut self) {
        self.selection_dismiss_at = None;
    }

    pub(crate) fn refresh_hovered_link(&mut self) {
        self.update_hover_input();
        let new_link = self.hovered_link_range();
        self.window.set_cursor(if new_link.is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        });
        if self.render_inputs.hovered_link != new_link {
            self.mark_dirty();
        }
    }

    pub(crate) fn hovered_link_range(&self) -> Option<HoveredLinkRange> {
        self.mux
            .hovered_link_range(self.active_pane)
            .map(|range| HoveredLinkRange {
                start: range.start,
                end: range.end,
            })
    }

    pub(crate) fn current_link_at_cursor(&mut self) -> Option<DetectedLink> {
        self.update_hover_input();
        self.mux.hovered_link(self.active_pane)
    }

    pub(crate) fn current_link_target(&mut self) -> Option<(LinkTarget, Option<String>)> {
        let link = self.current_link_at_cursor()?;
        let pwd = self
            .mux
            .pane_view(self.active_pane)
            .and_then(|view| view.pwd().map(str::to_owned));
        Some((link.target, pwd))
    }

    fn update_hover_input(&mut self) {
        let pos = self.current_hover_cell();
        let modifiers = self.link_modifiers();
        let _ = self.mux.set_hover_input(self.active_pane, pos, modifiers);
    }

    fn current_hover_cell(&self) -> GridPos {
        let (col, row) = self
            .renderer
            .pixel_to_grid(self.mouse.cursor_pos.x, self.mouse.cursor_pos.y);
        GridPos { col, row }
    }

    fn link_modifiers(&self) -> LinkModifiers {
        let state = self.modifiers.state();
        LinkModifiers {
            super_key: state.super_key(),
            ctrl: state.control_key(),
            alt: state.alt_key(),
            shift: state.shift_key(),
        }
    }

    pub(crate) fn paste_from_clipboard(&mut self) {
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
