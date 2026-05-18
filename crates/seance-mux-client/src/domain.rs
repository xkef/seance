use bytes::Bytes;
use seance_protocol::frame::{CursorShape, Resize, ThemeColors};
use seance_protocol::identity::PaneRef;

use crate::{DomainEvent, PaneError, SpawnError};

/// Default scrollback buffer size handed to fresh panes. Mirrors the VT
/// engine's default; lives at the mux layer so clients depending only on
/// `seance-mux-client` can construct `PaneSpawnOptions` without pulling in
/// `seance-vt`.
pub const DEFAULT_MAX_SCROLLBACK: usize = 10_000;

/// Knobs the client hands the server when spawning a new pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSpawnOptions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub initial_cursor_shape: CursorShape,
    pub max_scrollback: usize,
}

impl Default for PaneSpawnOptions {
    fn default() -> Self {
        Self {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
            initial_cursor_shape: CursorShape::Block,
            max_scrollback: DEFAULT_MAX_SCROLLBACK,
        }
    }
}

/// The seam every server-side actor implements. `LocalDomain` (in
/// `seance-mux-server`) wraps real PTYs; `ProtocolDomain` (here) talks to
/// a remote Domain over a `Transport`. Methods are intentionally
/// best-effort and synchronous; errors that target a specific pane are
/// reported through `drain_events` rather than as method returns where
/// possible.
pub trait Domain {
    /// Spawn a new pane and return its `PaneRef`. The client may treat the
    /// returned ref as opaque — only the Domain that minted it can resolve
    /// it back to local resources.
    fn spawn_pane(&mut self, options: PaneSpawnOptions) -> Result<PaneRef, SpawnError>;

    /// Pull every event the Domain has accumulated since the last call and
    /// hand each one to `sink`. The mux client batches these via
    /// `MuxClient::refresh_updates`.
    fn drain_events(&mut self, sink: &mut dyn FnMut(DomainEvent)) -> Result<(), PaneError>;

    /// Forward keyboard / paste input bytes to the pane's PTY.
    fn write(&mut self, pane: PaneRef, bytes: Bytes) -> Result<(), PaneError>;

    /// Notify the pane that its window-side dimensions changed; the VT
    /// reflow happens server-side and a `PaneUpdate` follows.
    fn resize(&mut self, pane: PaneRef, resize: Resize) -> Result<(), PaneError>;

    /// Scroll the pane's viewport `delta` lines (positive = toward older
    /// scrollback, negative = toward newer output).
    fn scroll_lines(&mut self, pane: PaneRef, delta: i32) -> Result<(), PaneError>;

    /// Replace the pane's effective color palette.
    fn set_theme_colors(&mut self, pane: PaneRef, colors: ThemeColors) -> Result<(), PaneError>;

    /// Override the cursor glyph; `None` reasserts the VT-reported shape.
    fn set_cursor_shape(&mut self, pane: PaneRef, shape: CursorShape) -> Result<(), PaneError>;

    /// Acknowledge that the host has rendered the frame at `generation`.
    /// Drives the DEC 2026 sync-output watchdog on the server side so the
    /// VT actor can release suppressed updates.
    fn ack_presented(&mut self, pane: PaneRef, generation: u64) -> Result<(), PaneError>;
}
