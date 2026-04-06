//! High-level terminal: VT emulator + PTY + selection.
//!
//! Uses libghostty-vt for terminal emulation. The raw GhosttyTerminal
//! handle is passed to the renderer via `raw_terminal_ptr()`.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use libghostty_vt::terminal::{Mode, ScrollViewport};
use libghostty_vt::{Terminal as VtTerminal, TerminalOptions};
use seance_pty::Pty;

use crate::selection::{self, GridPos, Selection, SelectionGranularity};

/// Terminal mode flags queried by the input encoder.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_event: i32,
    pub mouse_format_sgr: bool,
    pub synchronized_output: bool,
    pub bracketed_paste: bool,
}

/// A terminal session: VT emulator, PTY, and text selection state.
pub struct Terminal {
    vt: Box<VtTerminal<'static, 'static>>,
    response_buf: Rc<RefCell<Vec<u8>>>,
    pty: Pty,
    dirty: bool,
    selection: Option<Selection>,
}

impl Terminal {
    /// Spawn a new shell in a PTY with the given grid dimensions.
    pub fn spawn(cols: u16, rows: u16) -> Option<Self> {
        let mut vt = Box::new(
            VtTerminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: 10_000,
            })
            .ok()?,
        );

        let response_buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
        let buf = response_buf.clone();
        vt.on_pty_write(move |_term, data| {
            buf.borrow_mut().extend_from_slice(data);
        })
        .ok()?;

        let pty = Pty::spawn(seance_pty::Size { cols, rows }).ok()?;
        Some(Self {
            vt,
            response_buf,
            pty,
            dirty: false,
            selection: None,
        })
    }

    /// Read pending PTY output and feed it to the VT emulator.
    /// Returns true if new data arrived.
    pub fn poll(&mut self) -> bool {
        let mut buf = [0u8; 4096];
        let mut got_data = false;
        loop {
            match self.pty.read(&mut buf) {
                Ok(n) if n > 0 => {
                    self.vt.vt_write(&buf[..n]);
                    got_data = true;
                }
                _ => break,
            }
        }
        let responses = self.response_buf.take();
        if !responses.is_empty() {
            let _ = self.pty.write_all(&responses);
        }
        if got_data {
            self.dirty = true;
        }
        got_data
    }

    /// Write raw bytes to the PTY (keyboard input, paste, etc.).
    pub fn write(&self, data: &[u8]) {
        let _ = self.pty.write_all(data);
    }

    /// Check whether the shell process is still running.
    pub fn is_alive(&mut self) -> bool {
        self.pty.is_alive()
    }

    /// Resize both the VT grid and the PTY window.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.vt.resize(cols, rows, 0, 0);
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    /// Scroll the viewport by `delta` lines (negative = up).
    pub fn scroll_lines(&mut self, delta: i32) {
        self.vt.scroll_viewport(ScrollViewport::Delta(delta as isize));
    }

    /// Query the current terminal mode flags.
    pub fn modes(&self) -> TerminalModes {
        TerminalModes {
            cursor_keys: self.vt.mode(Mode::DECCKM).unwrap_or(false),
            mouse_event: if self.vt.is_mouse_tracking().unwrap_or(false) { 1000 } else { 0 },
            mouse_format_sgr: self.vt.mode(Mode::SGR_MOUSE).unwrap_or(false),
            synchronized_output: self.vt.mode(Mode::SYNC_OUTPUT).unwrap_or(false),
            bracketed_paste: self.vt.mode(Mode::BRACKETED_PASTE).unwrap_or(false),
        }
    }

    // -- Selection ------------------------------------------------------------

    /// Start a character-level selection at the given grid cell.
    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    /// Start a word-level selection (double-click).
    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new_word(GridPos { col, row }));
    }

    /// Start a line-level selection (triple-click).
    pub fn start_line_selection(&mut self, row: u16) {
        self.selection = Some(Selection::new_line(GridPos { col: 0, row }));
    }

    /// Update the moving end of the current selection.
    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(sel) = &mut self.selection {
            sel.update(GridPos { col, row });
        }
    }

    /// Extract the selected text. Returns `None` if no selection is active.
    ///
    /// Word selections are expanded to word boundaries. The text is built
    /// from the VT screen content (currently a placeholder — requires
    /// `dump_screen` implementation).
    pub fn selection_text(&mut self) -> Option<String> {
        let cols = self.vt.cols().unwrap_or(80);
        // TODO: use libghostty-vt RenderState row/cell iteration
        let screen = String::new();
        let lines: Vec<&str> = screen.lines().collect();
        let sel = self.selection.as_ref()?;
        if sel.granularity() == SelectionGranularity::Word {
            let (start, end) = sel.ordered_range();
            let mut expanded = sel.clone();
            if let Some(line) = lines.get(start.row as usize) {
                let (ws, _) = selection::word_boundaries(line, start.col);
                expanded.update(GridPos { col: ws, row: start.row });
            }
            if let Some(line) = lines.get(end.row as usize) {
                let (_, we) = selection::word_boundaries(line, end.col);
                expanded.update(GridPos { col: we, row: end.row });
            }
            return Some(expanded.extract_text(&lines, cols));
        }
        Some(sel.extract_text(&lines, cols))
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Get the normalized (start, end) range of the current selection.
    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(|s| s.ordered_range())
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Select the entire visible grid.
    pub fn select_all(&mut self) {
        let cols = self.vt.cols().unwrap_or(80);
        let rows = self.vt.rows().unwrap_or(24);
        self.selection = Some(Selection::new_line(GridPos { col: 0, row: 0 }));
        if let Some(sel) = &mut self.selection {
            sel.update(GridPos { col: cols.saturating_sub(1), row: rows.saturating_sub(1) });
        }
    }

    /// Raw libghostty terminal pointer for the renderer.
    pub(crate) fn raw_terminal_ptr(&self) -> *mut c_void {
        self.vt.as_raw().cast()
    }
}
