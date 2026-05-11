use bytes::Bytes;
use seance_protocol::frame::{CursorShape, Resize, ThemeColors};
use seance_protocol::identity::PaneRef;

use crate::{DomainEvent, PaneError, SpawnError};

/// Options the [`Domain`] uses when spawning a new pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpawnOptions {
    /// Initial grid width in cells.
    pub cols: u16,
    /// Initial grid height in cells.
    pub rows: u16,
    /// Initial cell width in pixels (used for kitty-graphics sizing).
    pub pixel_width: u16,
    /// Initial cell height in pixels (used for kitty-graphics sizing).
    pub pixel_height: u16,
    /// Cursor shape the spawned pane presents until DECSCUSR overrides
    /// it.
    pub initial_cursor_shape: CursorShape,
}

impl Default for PaneSpawnOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
            initial_cursor_shape: CursorShape::Block,
        }
    }
}

impl From<PaneSpawnOptions> for seance_vt::VtSessionOptions {
    fn from(value: PaneSpawnOptions) -> Self {
        Self {
            cols: value.cols,
            rows: value.rows,
            pixel_width: value.pixel_width,
            pixel_height: value.pixel_height,
            initial_cursor_shape: value.initial_cursor_shape,
        }
    }
}

/// Backend the [`crate::MuxClient`] talks to. A domain owns one or
/// more panes and translates client calls into pane operations —
/// either in-process (see [`crate::LocalDomain`]) or remote (see
/// [`crate::ProtocolDomain`]).
pub trait Domain {
    /// Spawn a new pane. Implementations may return synchronously with
    /// a [`PaneRef`] (local) or surface the new pane later via
    /// [`Self::drain_events`] (remote).
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError>;

    /// Drive any pending domain-side work and forward each resulting
    /// [`DomainEvent`] to `sink`.
    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError>;

    /// Send PTY input bytes to `pane`.
    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError>;

    /// Resize `pane`.
    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError>;

    /// Scroll `pane` by `delta` rows (negative scrolls back).
    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError>;

    /// Replace `pane`'s palette.
    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError>;

    /// Override `pane`'s cursor shape.
    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError>;

    /// Acknowledge that frame `generation` reached the screen so the
    /// domain can reset the dirty extent published before that frame.
    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError>;
}
