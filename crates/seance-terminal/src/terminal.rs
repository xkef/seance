//! High-level terminal: pairs a VT emulator with a PTY and provides
//! search, selection, and screen content APIs.
//!
//! This is the unit that the multiplexer operates on — each pane owns
//! one `Terminal`. It knows nothing about GPU rendering; the
//! [`TerminalRenderer`](crate::TerminalRenderer) handles that.

use ghostty_renderer::{self as gr, ScrollAction};
use seance_pty::Pty;

use crate::search::SearchState;
use crate::selection::{self, GridPos, Selection, SelectionGranularity};

/// Terminal modes that affect input encoding.
///
/// Consumers (like an input handler) use these to decide how
/// to encode keystrokes and mouse events.
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    /// DECCKM (application cursor keys) is active.
    pub cursor_keys: bool,
    /// Active mouse tracking mode (0 = none, 9 = X10, 1000/1002/1003).
    pub mouse_event: i32,
    /// SGR mouse format (mode 1006) is active.
    pub mouse_format_sgr: bool,
    /// Synchronized output (mode 2026) is active.
    pub synchronized_output: bool,
}

/// Cursor position and visibility.
#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

/// A managed terminal pane with VT emulation, PTY I/O, search, and selection.
///
/// # Usage
///
/// ```no_run
/// use seance_terminal::Terminal;
///
/// let mut term = Terminal::spawn(80, 24).unwrap();
/// term.write(b"ls -la\r");
/// term.poll();
/// println!("{}", term.dump_screen());
/// ```
pub struct Terminal {
    vt: gr::Terminal,
    pty: Pty,
    dirty: bool,
    search: SearchState,
    selection: Option<Selection>,
}

impl Terminal {
    /// Spawn a new terminal with a login shell.
    ///
    /// Creates a VT emulator with the given grid dimensions and a PTY
    /// running the user's `$SHELL` (or `/bin/sh`).
    pub fn spawn(cols: u16, rows: u16) -> Option<Self> {
        let vt = gr::Terminal::new(cols, rows)?;
        let pty = Pty::spawn(seance_pty::Size { cols, rows }).ok()?;
        Some(Self {
            vt,
            pty,
            dirty: false,
            search: SearchState::new(),
            selection: None,
        })
    }

    // -- I/O --

    /// Read all available PTY output into the VT emulator.
    ///
    /// Returns `true` if any bytes were received. Call this once per
    /// frame (or event iteration) to keep the terminal up to date.
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
        // Drain device-status responses back to the PTY.
        let responses = self.vt.drain_responses();
        if !responses.is_empty() {
            let _ = self.pty.write_all(responses);
        }
        self.vt.clear_responses();
        if got_data {
            self.dirty = true;
        }
        got_data
    }

    /// Write raw bytes to the PTY (keyboard input).
    pub fn write(&self, data: &[u8]) {
        let _ = self.pty.write_all(data);
    }

    /// Whether the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        self.pty.is_alive()
    }

    // -- Grid --

    /// Resize the terminal grid and PTY.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.vt.resize(cols, rows);
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    /// Current grid dimensions `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        self.vt.size()
    }

    // -- Cursor --

    /// Current cursor position and visibility.
    pub fn cursor(&self) -> CursorState {
        let c = self.vt.cursor();
        CursorState {
            col: c.col,
            row: c.row,
            visible: c.visible,
        }
    }

    // -- Title --

    /// Terminal title set via OSC 0/2. Returns `None` if unset.
    pub fn title(&self) -> Option<String> {
        self.vt.title()
    }

    // -- Scrolling --

    /// Scroll the terminal viewport.
    pub fn scroll(&mut self, action: ScrollAction) {
        self.vt.scroll(action);
    }

    /// Scroll to the very top of the scrollback buffer.
    pub fn scroll_to_top(&mut self) {
        self.vt.scroll(ScrollAction::Top);
    }

    /// Scroll to the bottom (live output).
    pub fn scroll_to_bottom(&mut self) {
        self.vt.scroll(ScrollAction::Bottom);
    }

    /// Number of scrollback rows currently available.
    pub fn scrollback_rows(&self) -> usize {
        self.vt.scrollback_rows()
    }

    // -- Modes --

    /// Query terminal modes for input encoding decisions.
    pub fn modes(&self) -> TerminalModes {
        TerminalModes {
            cursor_keys: self.vt.mode_cursor_keys(),
            mouse_event: self.vt.mode_mouse_event(),
            mouse_format_sgr: self.vt.mode_mouse_format_sgr(),
            synchronized_output: self.vt.mode_synchronized_output(),
        }
    }

    // -- Screen content --

    /// Dump the visible screen content as UTF-8 text.
    pub fn dump_screen(&mut self) -> String {
        self.vt.dump_screen()
    }

    // -- Dirty tracking --

    /// Returns `true` if new PTY data has arrived since the last call,
    /// and resets the flag.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    // -- Search --

    /// Search the visible screen for `query` (case-insensitive).
    ///
    /// Returns the number of matches found. Use [`search_next`] and
    /// [`search_prev`] to cycle through them.
    pub fn search(&mut self, query: &str) -> usize {
        let (cols, _) = self.vt.size();
        let screen = self.vt.dump_screen();
        self.search.search(query, &screen, cols)
    }

    /// Move to the next search match.
    pub fn search_next(&mut self) -> Option<crate::search::SearchMatch> {
        self.search.next()
    }

    /// Move to the previous search match.
    pub fn search_prev(&mut self) -> Option<crate::search::SearchMatch> {
        self.search.prev()
    }

    /// The currently highlighted search match.
    pub fn current_search_match(&self) -> Option<crate::search::SearchMatch> {
        self.search.current_match()
    }

    /// All matches from the most recent search.
    pub fn search_matches(&self) -> &[crate::search::SearchMatch] {
        self.search.matches()
    }

    /// Clear the search state.
    pub fn clear_search(&mut self) {
        self.search.clear();
    }

    // -- Selection --

    /// Start a character-level selection at the given grid position.
    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    /// Start a word-level selection (e.g. double-click).
    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new_word(GridPos { col, row }));
    }

    /// Start a line-level selection (e.g. triple-click).
    pub fn start_line_selection(&mut self, row: u16) {
        self.selection = Some(Selection::new_line(GridPos { col: 0, row }));
    }

    /// Update the moving end of the selection.
    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(sel) = &mut self.selection {
            sel.update(GridPos { col, row });
        }
    }

    /// Extract the selected text from the screen. Returns `None` if
    /// there is no active selection.
    pub fn selection_text(&mut self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (cols, _) = self.vt.size();
        let screen = self.vt.dump_screen();
        let lines: Vec<&str> = screen.lines().collect();

        // For word selection, expand boundaries.
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

    /// Whether a selection is currently active.
    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// The current selection range (start, end) in normalized order.
    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(|s| s.ordered_range())
    }

    /// Clear the current selection.
    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Select all visible screen content.
    pub fn select_all(&mut self) {
        let (cols, rows) = self.vt.size();
        self.selection = Some(Selection::new_line(GridPos { col: 0, row: 0 }));
        if let Some(sel) = &mut self.selection {
            sel.update(GridPos {
                col: cols.saturating_sub(1),
                row: rows.saturating_sub(1),
            });
        }
    }

    // -- Low-level access (for the renderer) --

    /// Borrow the underlying ghostty terminal.
    ///
    /// This is needed by [`TerminalRenderer`](crate::TerminalRenderer) to
    /// attach terminals and read cell buffers. Not intended for general use.
    pub(crate) fn vt(&self) -> &gr::Terminal {
        &self.vt
    }
}
