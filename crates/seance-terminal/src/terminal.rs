//! High-level terminal: VT emulator + PTY + search + selection.
//!
//! Uses libghostty-vt for terminal emulation. The raw GhosttyTerminal
//! handle is passed to the renderer via `raw_terminal_ptr()`.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use libghostty_vt::terminal::{Mode, ScrollViewport};
use libghostty_vt::{Terminal as VtTerminal, TerminalOptions};
use seance_pty::Pty;

use crate::search::SearchState;
use crate::selection::{self, GridPos, Selection, SelectionGranularity};

#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_event: i32,
    pub mouse_format_sgr: bool,
    pub synchronized_output: bool,
    pub bracketed_paste: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

pub struct Terminal {
    vt: Box<VtTerminal<'static, 'static>>,
    response_buf: Rc<RefCell<Vec<u8>>>,
    pty: Pty,
    dirty: bool,
    search: SearchState,
    selection: Option<Selection>,
}

impl Terminal {
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
            search: SearchState::new(),
            selection: None,
        })
    }

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

    pub fn write(&self, data: &[u8]) {
        let _ = self.pty.write_all(data);
    }

    pub fn is_alive(&mut self) -> bool {
        self.pty.is_alive()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let _ = self.vt.resize(cols, rows, 0, 0);
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    pub fn resize_pty(&mut self, cols: u16, rows: u16) {
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    pub fn resize_vt(&mut self, cols: u16, rows: u16) {
        let _ = self.vt.resize(cols, rows, 0, 0);
    }

    pub fn size(&self) -> (u16, u16) {
        let cols = self.vt.cols().unwrap_or(80);
        let rows = self.vt.rows().unwrap_or(24);
        (cols, rows)
    }

    pub fn cursor(&self) -> CursorState {
        CursorState {
            col: self.vt.cursor_x().unwrap_or(0),
            row: self.vt.cursor_y().unwrap_or(0),
            visible: self.vt.is_cursor_visible().unwrap_or(true),
        }
    }

    pub fn title(&self) -> Option<String> {
        let t = self.vt.title().ok()?;
        if t.is_empty() { None } else { Some(t.to_owned()) }
    }

    pub fn scroll(&mut self, sv: ScrollViewport) {
        self.vt.scroll_viewport(sv);
    }

    pub fn scroll_lines(&mut self, delta: i32) {
        self.vt.scroll_viewport(ScrollViewport::Delta(delta as isize));
    }

    pub fn scroll_page_up(&mut self) {
        let (_, rows) = self.size();
        self.vt.scroll_viewport(ScrollViewport::Delta(-(rows as isize)));
    }

    pub fn scroll_page_down(&mut self) {
        let (_, rows) = self.size();
        self.vt.scroll_viewport(ScrollViewport::Delta(rows as isize));
    }

    pub fn scroll_to_top(&mut self) {
        self.vt.scroll_viewport(ScrollViewport::Top);
    }

    pub fn scroll_to_bottom(&mut self) {
        self.vt.scroll_viewport(ScrollViewport::Bottom);
    }

    pub fn scrollback_rows(&self) -> usize {
        self.vt.scrollback_rows().unwrap_or(0)
    }

    pub fn modes(&self) -> TerminalModes {
        TerminalModes {
            cursor_keys: self.vt.mode(Mode::DECCKM).unwrap_or(false),
            mouse_event: if self.vt.is_mouse_tracking().unwrap_or(false) { 1000 } else { 0 },
            mouse_format_sgr: self.vt.mode(Mode::SGR_MOUSE).unwrap_or(false),
            synchronized_output: self.vt.mode(Mode::SYNC_OUTPUT).unwrap_or(false),
            bracketed_paste: self.vt.mode(Mode::BRACKETED_PASTE).unwrap_or(false),
        }
    }

    pub fn dump_screen(&mut self) -> String {
        // TODO: use libghostty-vt RenderState row/cell iteration
        String::new()
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn search(&mut self, query: &str) -> usize {
        let (cols, _) = self.size();
        let screen = self.dump_screen();
        self.search.search(query, &screen, cols)
    }
    pub fn search_next(&mut self) -> Option<crate::search::SearchMatch> { self.search.next() }
    pub fn search_prev(&mut self) -> Option<crate::search::SearchMatch> { self.search.prev() }
    pub fn current_search_match(&self) -> Option<crate::search::SearchMatch> { self.search.current_match() }
    pub fn search_matches(&self) -> &[crate::search::SearchMatch] { self.search.matches() }
    pub fn clear_search(&mut self) { self.search.clear(); }

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
        if let Some(sel) = &mut self.selection { sel.update(GridPos { col, row }); }
    }
    pub fn selection_text(&mut self) -> Option<String> {
        let (cols, _) = self.size();
        let screen = self.dump_screen();
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
    pub fn has_selection(&self) -> bool { self.selection.is_some() }
    pub fn selection_range(&self) -> Option<(GridPos, GridPos)> {
        self.selection.as_ref().map(|s| s.ordered_range())
    }
    pub fn clear_selection(&mut self) { self.selection = None; }
    pub fn select_all(&mut self) {
        let (cols, rows) = self.size();
        self.selection = Some(Selection::new_line(GridPos { col: 0, row: 0 }));
        if let Some(sel) = &mut self.selection {
            sel.update(GridPos { col: cols.saturating_sub(1), row: rows.saturating_sub(1) });
        }
    }

    pub(crate) fn raw_terminal_ptr(&self) -> *mut c_void {
        self.vt.as_raw().cast()
    }
}
