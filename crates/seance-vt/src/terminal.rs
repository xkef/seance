//! High-level terminal: VT emulator + PTY + selection.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;

use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::terminal::{Mode, ScrollViewport};
use libghostty_vt::{RenderState, Terminal as VtTerminal, TerminalOptions};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::modes::TerminalModes;
use crate::selection::{GridPos, Selection, SelectionGranularity};

/// A terminal session: VT emulator, PTY, and text selection state.
pub struct Terminal {
    vt: Box<VtTerminal<'static, 'static>>,
    response_buf: Rc<RefCell<Vec<u8>>>,
    reader: Box<dyn Read + Send>,
    writer: RefCell<Box<dyn Write + Send>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    dirty: bool,
    selection: Option<Selection>,
}

impl Terminal {
    /// Spawn a new shell in a PTY with the given grid and pixel dimensions.
    pub fn spawn(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Option<Self> {
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

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width,
                pixel_height,
            })
            .ok()?;

        let cmd = CommandBuilder::new_default_prog();
        let child = pair.slave.spawn_command(cmd).ok()?;
        let reader = pair.master.try_clone_reader().ok()?;
        let writer = pair.master.take_writer().ok()?;

        // Set the master fd to non-blocking so poll() doesn't block the
        // event loop. The cloned reader inherits the non-blocking flag.
        #[cfg(unix)]
        if let Some(fd) = pair.master.as_raw_fd() {
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
            }
        }

        Some(Self {
            vt,
            response_buf,
            reader,
            writer: RefCell::new(writer),
            master: pair.master,
            child,
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
            match self.reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    self.vt.vt_write(&buf[..n]);
                    got_data = true;
                }
                _ => break,
            }
        }
        let responses = self.response_buf.take();
        if !responses.is_empty() {
            let _ = self.writer.borrow_mut().write_all(&responses);
        }
        if got_data {
            self.dirty = true;
        }
        got_data
    }

    /// Write raw bytes to the PTY (keyboard input, paste, etc.).
    pub fn write(&self, data: &[u8]) {
        let _ = self.writer.borrow_mut().write_all(data);
    }

    /// Check whether the shell process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Resize both the VT grid and the PTY window.
    pub fn resize(&mut self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) {
        let _ = self.vt.resize(cols, rows, 0, 0);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        });
    }

    /// Scroll the viewport by `delta` lines (negative = up).
    pub fn scroll_lines(&mut self, delta: i32) {
        self.vt
            .scroll_viewport(ScrollViewport::Delta(delta as isize));
    }

    /// Query the current terminal mode flags.
    pub fn modes(&self) -> TerminalModes {
        TerminalModes {
            cursor_keys: self.vt.mode(Mode::DECCKM).unwrap_or(false),
            mouse_event: if self.vt.is_mouse_tracking().unwrap_or(false) {
                1000
            } else {
                0
            },
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

    /// Extract the selected text using libghostty-vt's RenderState API.
    /// Returns `None` if no selection is active.
    pub fn selection_text(&mut self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.ordered_range();
        let granularity = sel.granularity();
        let cols = self.vt.cols().unwrap_or(80);

        let mut render_state = RenderState::new().ok()?;
        let snapshot = render_state.update(&self.vt).ok()?;
        let mut rows = RowIterator::new().ok()?;
        let mut cells = CellIterator::new().ok()?;

        let mut result = String::new();
        let mut row_iter = rows.update(&snapshot).ok()?;
        let mut row_idx: u16 = 0;

        while let Some(row) = row_iter.next() {
            if row_idx > end.row {
                break;
            }
            if row_idx >= start.row {
                let col_start = match granularity {
                    SelectionGranularity::Line => 0,
                    _ if row_idx == start.row => start.col,
                    _ => 0,
                };
                let col_end = match granularity {
                    SelectionGranularity::Line => cols.saturating_sub(1),
                    _ if row_idx == end.row => end.col,
                    _ => cols.saturating_sub(1),
                };

                if !result.is_empty() {
                    result.push('\n');
                }

                let mut cell_iter = cells.update(row).ok()?;
                let mut col_idx: u16 = 0;
                while let Some(cell) = cell_iter.next() {
                    if col_idx >= col_start && col_idx <= col_end {
                        let graphemes = cell.graphemes().ok()?;
                        if graphemes.is_empty() {
                            result.push(' ');
                        } else {
                            for ch in graphemes {
                                result.push(ch);
                            }
                        }
                    }
                    col_idx += 1;
                }

                // Trim trailing whitespace from each line.
                let trimmed_len = result.trim_end().len();
                result.truncate(trimmed_len);
            }
            row_idx += 1;
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
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
            sel.update(GridPos {
                col: cols.saturating_sub(1),
                row: rows.saturating_sub(1),
            });
        }
    }

    /// Expose the underlying VT emulator to the renderer.
    ///
    /// Temporary surface used by the phase-3 renderer; the phase-4
    /// `FrameSource` trait will replace this with a narrower view.
    pub fn vt_mut(&mut self) -> &mut VtTerminal<'static, 'static> {
        &mut self.vt
    }
}
