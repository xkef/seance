//! Test-only, PTY-less Headless VT.
//!
//! `#[doc(hidden)]`. Not a stable API; the only consumer is
//! `seance-render-test`. Do not widen without coordinating with the
//! harness crate.

use libghostty_vt::terminal::Mode;

use crate::VtCoreError;
use crate::core::{DEFAULT_MAX_SCROLLBACK, VtCore, VtCoreOptions};
use crate::frame::CursorShape;
use crate::snapshot::VtSnapshot;

/// A PTY-less VT adapter for tests.
pub struct HeadlessTerminal {
    core: VtCore,
}

impl HeadlessTerminal {
    /// Build a new `cols × rows` Headless VT. Returns `None` if
    /// libghostty-vt rejects the configuration.
    pub fn new(cols: u16, rows: u16) -> Option<Self> {
        let core = VtCore::new(VtCoreOptions {
            cols,
            rows,
            max_scrollback: DEFAULT_MAX_SCROLLBACK,
            pixel_width: 0,
            pixel_height: 0,
            initial_cursor_shape: CursorShape::Block,
        })
        .ok()?;
        Some(Self { core })
    }

    /// Feed raw VT bytes (escape sequences, UTF-8, etc.).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.core.feed(bytes);
    }

    /// Drain any VT-originated response bytes accumulated since the
    /// last call.
    pub fn take_responses(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for bytes in self.core.drain_responses() {
            out.extend_from_slice(&bytes);
        }
        out
    }

    pub fn snapshot(&mut self) -> Result<VtSnapshot, VtCoreError> {
        self.core.snapshot()
    }

    pub fn ack_rendered(&mut self, generation: u64) {
        self.core.ack_rendered(generation);
    }

    pub fn cols(&self) -> u16 {
        self.core.cols()
    }

    pub fn rows(&self) -> u16 {
        self.core.rows()
    }

    pub fn cursor_pos(&self) -> (u16, u16) {
        self.core.cursor_pos()
    }

    pub fn is_cursor_visible(&self) -> bool {
        self.core.is_cursor_visible()
    }

    /// Scroll the viewport by `delta` rows. Negative values scroll up into the
    /// scrollback, positive values scroll back down toward the active screen.
    pub fn scroll_lines(&mut self, delta: i32) {
        self.core.scroll_lines(delta);
    }

    pub fn mode(&self, m: Mode) -> bool {
        self.core.mode(m)
    }
}
