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

const READ_CHUNK: usize = 4096;
const MAX_SCROLLBACK: usize = 10_000;

/// A terminal session: VT emulator, PTY, and text selection state.
pub struct Terminal {
    vt: Box<VtTerminal<'static, 'static>>,
    response_buf: Rc<RefCell<Vec<u8>>>,
    reader: Box<dyn Read + Send>,
    writer: RefCell<Box<dyn Write + Send>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    selection: Option<Selection>,
}

impl Terminal {
    /// Spawn a new shell in a PTY.
    pub fn spawn(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Option<Self> {
        let mut vt = Box::new(
            VtTerminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: MAX_SCROLLBACK,
            })
            .ok()?,
        );

        let response_buf = Rc::new(RefCell::new(Vec::new()));
        let buf = Rc::clone(&response_buf);
        vt.on_pty_write(move |_, data| buf.borrow_mut().extend_from_slice(data))
            .ok()?;

        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width,
                pixel_height,
            })
            .ok()?;

        let child = pair
            .slave
            .spawn_command(CommandBuilder::new_default_prog())
            .ok()?;
        let reader = pair.master.try_clone_reader().ok()?;
        let writer = pair.master.take_writer().ok()?;

        // Non-blocking so poll() never blocks the event loop.
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
            selection: None,
        })
    }

    /// Read pending PTY output, feed the VT, flush device responses.
    /// Returns true if new data arrived.
    pub fn poll(&mut self) -> bool {
        let mut buf = [0u8; READ_CHUNK];
        let mut got_data = false;
        while let Ok(n) = self.reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            self.vt.vt_write(&buf[..n]);
            got_data = true;
        }
        let responses = self.response_buf.take();
        if !responses.is_empty() {
            let _ = self.writer.borrow_mut().write_all(&responses);
        }
        got_data
    }

    /// Write raw bytes to the PTY (keyboard input, paste, etc.).
    pub fn write(&self, data: &[u8]) {
        let _ = self.writer.borrow_mut().write_all(data);
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn resize(&mut self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) {
        let _ = self.vt.resize(cols, rows, 0, 0);
        let _ = self.master.resize(PtySize {
            rows,
            cols,
            pixel_width,
            pixel_height,
        });
    }

    pub fn scroll_lines(&mut self, delta: i32) {
        self.vt
            .scroll_viewport(ScrollViewport::Delta(delta as isize));
    }

    pub fn modes(&self) -> TerminalModes {
        let mode = |m| self.vt.mode(m).unwrap_or(false);
        TerminalModes {
            cursor_keys: mode(Mode::DECCKM),
            mouse_tracking: self.vt.is_mouse_tracking().unwrap_or(false),
            mouse_format_sgr: mode(Mode::SGR_MOUSE),
            bracketed_paste: mode(Mode::BRACKETED_PASTE),
        }
    }

    // -- Selection ----------------------------------------------------

    pub fn start_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new(GridPos { col, row }));
    }

    pub fn start_word_selection(&mut self, col: u16, row: u16) {
        self.selection = Some(Selection::new_word(GridPos { col, row }));
    }

    pub fn start_line_selection(&mut self, row: u16) {
        self.selection = Some(Selection::new_line(GridPos { col: 0, row }));
    }

    pub fn update_selection(&mut self, col: u16, row: u16) {
        if let Some(sel) = &mut self.selection {
            sel.update(GridPos { col, row });
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(Selection::ordered_range)
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    /// Select the entire visible grid as a line selection.
    pub fn select_all(&mut self) {
        let cols = self.vt.cols().unwrap_or(80);
        let rows = self.vt.rows().unwrap_or(24);
        let mut sel = Selection::new_line(GridPos { col: 0, row: 0 });
        sel.update(GridPos {
            col: cols.saturating_sub(1),
            row: rows.saturating_sub(1),
        });
        self.selection = Some(sel);
    }

    /// Extract the selected text from the live VT grid.
    pub fn selection_text(&mut self) -> Option<String> {
        let sel = self.selection.as_ref()?;
        let (start, end) = sel.ordered_range();
        let granularity = sel.granularity();
        let cols = self.vt.cols().unwrap_or(80);

        let mut render_state = RenderState::new().ok()?;
        let snapshot = render_state.update(&self.vt).ok()?;
        let mut rows = RowIterator::new().ok()?;
        let mut cells = CellIterator::new().ok()?;
        let mut row_iter = rows.update(&snapshot).ok()?;

        let mut out = String::new();
        let mut row_idx: u16 = 0;
        while let Some(row) = row_iter.next() {
            if row_idx > end.row {
                break;
            }
            if row_idx >= start.row {
                let (col_start, col_end) = column_range(granularity, row_idx, start, end, cols);
                if !out.is_empty() {
                    out.push('\n');
                }

                let mut cell_iter = cells.update(row).ok()?;
                let mut col: u16 = 0;
                while let Some(cell) = cell_iter.next() {
                    if col >= col_start && col <= col_end {
                        let graphemes = cell.graphemes().ok()?;
                        if graphemes.is_empty() {
                            out.push(' ');
                        } else {
                            out.extend(graphemes);
                        }
                    }
                    col += 1;
                }

                let trimmed = out.trim_end().len();
                out.truncate(trimmed);
            }
            row_idx += 1;
        }

        if out.is_empty() { None } else { Some(out) }
    }

    pub(crate) fn vt_mut(&mut self) -> &mut VtTerminal<'static, 'static> {
        &mut self.vt
    }
}

fn column_range(
    granularity: SelectionGranularity,
    row_idx: u16,
    start: GridPos,
    end: GridPos,
    cols: u16,
) -> (u16, u16) {
    let last = cols.saturating_sub(1);
    match granularity {
        SelectionGranularity::Line => (0, last),
        _ => {
            let cs = if row_idx == start.row { start.col } else { 0 };
            let ce = if row_idx == end.row { end.col } else { last };
            (cs, ce)
        }
    }
}
