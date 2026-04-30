//! Test-only, PTY-less VT emulator.
//!
//! `#[doc(hidden)]`. Not a stable API; the only consumer is
//! `seance-render-test`. Do not widen without coordinating with the
//! harness crate.

use std::sync::{Arc, Mutex};

use libghostty_vt::terminal::Mode;
use libghostty_vt::{Terminal as VtTerminal, TerminalOptions};

use crate::frame::{
    CellVisitor, CursorInfo, FrameSource, ImageVisitor, PlacementLayer, PlacementVisitor,
};
use crate::frame_source::walk_vt_cells;
use crate::selection::GridPos;

const MAX_SCROLLBACK: usize = 10_000;

/// A libghostty-vt terminal with no PTY and no child process.
///
/// Bytes are fed directly into the VT state machine via [`Self::feed`];
/// VT-originated responses (e.g. to CSI DA queries) accumulate in an
/// internal buffer retrievable with [`Self::take_responses`].
///
/// Implements [`FrameSource`] so the rendering-adjacent iteration
/// logic in `seance-render-test` can walk a cell grid identically to
/// the production `LibGhosttyFrameSource`.
pub struct HeadlessTerminal {
    vt: Box<VtTerminal<'static, 'static>>,
    responses: Arc<Mutex<Vec<u8>>>,
}

impl HeadlessTerminal {
    /// Build a new `cols × rows` headless terminal. Returns `None` if
    /// libghostty-vt rejects the configuration.
    pub fn new(cols: u16, rows: u16) -> Option<Self> {
        let mut vt = Box::new(
            VtTerminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: MAX_SCROLLBACK,
            })
            .ok()?,
        );
        // Cell pixel dims are irrelevant for tests that only feed VT bytes;
        // virtual-placement rendering is out of scope for the harness.
        vt.resize(cols, rows, 0, 0).ok()?;

        let responses = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&responses);
        vt.on_pty_write(move |_, data| {
            sink.lock()
                .expect("headless terminal response buffer mutex poisoned")
                .extend_from_slice(data);
        })
        .ok()?;

        Some(Self { vt, responses })
    }

    /// Feed raw VT bytes (escape sequences, UTF-8, etc.).
    pub fn feed(&mut self, bytes: &[u8]) {
        self.vt.vt_write(bytes);
    }

    /// Drain any VT-originated response bytes accumulated since the
    /// last call.
    pub fn take_responses(&self) -> Vec<u8> {
        let mut responses = self
            .responses
            .lock()
            .expect("headless terminal response buffer mutex poisoned");
        std::mem::take(&mut *responses)
    }

    pub fn cols(&self) -> u16 {
        self.vt.cols().unwrap_or(0)
    }

    pub fn rows(&self) -> u16 {
        self.vt.rows().unwrap_or(0)
    }

    pub fn cursor_pos(&self) -> (u16, u16) {
        (
            self.vt.cursor_x().unwrap_or(0),
            self.vt.cursor_y().unwrap_or(0),
        )
    }

    pub fn is_cursor_visible(&self) -> bool {
        self.vt.is_cursor_visible().unwrap_or(true)
    }

    pub fn mode(&self, m: Mode) -> bool {
        self.vt.mode(m).unwrap_or(false)
    }
}

impl FrameSource for HeadlessTerminal {
    fn grid_size(&mut self) -> (u16, u16) {
        (self.vt.cols().unwrap_or(80), self.vt.rows().unwrap_or(24))
    }

    fn cursor(&mut self) -> CursorInfo {
        CursorInfo {
            pos: GridPos {
                col: self.vt.cursor_x().unwrap_or(0),
                row: self.vt.cursor_y().unwrap_or(0),
            },
            visible: self.vt.is_cursor_visible().unwrap_or(true),
            wide: false,
            shape: None,
        }
    }

    fn selection(&mut self) -> Option<(GridPos, GridPos)> {
        None
    }

    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor) {
        let _ = walk_vt_cells(&mut self.vt, visitor);
    }

    fn visit_placements(&mut self, _layer: PlacementLayer, _visitor: &mut dyn PlacementVisitor) {}

    fn visit_images(&mut self, _visitor: &mut dyn ImageVisitor) {}
}
