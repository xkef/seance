//! High-level terminal: VT emulator + PTY + search + selection.
//!
//! Uses libghostty-vt-sys for terminal emulation. The raw GhosttyTerminal
//! handle is passed to the renderer via `raw_terminal_ptr()`.

use std::ffi::c_void;
use std::ptr::NonNull;

use libghostty_vt_sys as ffi;
use seance_pty::Pty;

use crate::search::SearchState;
use crate::selection::{self, GridPos, Selection, SelectionGranularity};

#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_event: i32,
    pub mouse_format_sgr: bool,
    pub synchronized_output: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct CursorState {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum ScrollAction {
    Lines(i32),
    Top,
    Bottom,
    PageUp,
    PageDown,
}

struct CallbackCtx {
    response_buf: Vec<u8>,
}

pub struct Terminal {
    raw: NonNull<ffi::TerminalImpl>,
    ctx: Box<CallbackCtx>,
    pty: Pty,
    dirty: bool,
    search: SearchState,
    selection: Option<Selection>,
}

const MODE_DECCKM: ffi::Mode = 1;
const MODE_SGR_MOUSE: ffi::Mode = 1006;
const MODE_SYNC_OUTPUT: ffi::Mode = 2026;

unsafe extern "C" fn pty_write_callback(
    _terminal: ffi::Terminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    unsafe {
        let ctx = &mut *(userdata as *mut CallbackCtx);
        let slice = std::slice::from_raw_parts(data, len);
        ctx.response_buf.extend_from_slice(slice);
    }
}

impl Terminal {
    pub fn spawn(cols: u16, rows: u16) -> Option<Self> {
        let mut raw: ffi::Terminal = std::ptr::null_mut();
        let opts = ffi::TerminalOptions {
            cols,
            rows,
            max_scrollback: 10_000,
        };
        let result = unsafe { ffi::ghostty_terminal_new(std::ptr::null(), &mut raw, opts) };
        if result != ffi::Result::SUCCESS {
            return None;
        }
        let raw = NonNull::new(raw)?;

        let ctx = Box::new(CallbackCtx {
            response_buf: Vec::new(),
        });

        unsafe {
            let ctx_ptr: *const c_void = (&*ctx as *const CallbackCtx).cast();
            ffi::ghostty_terminal_set(
                raw.as_ptr(),
                ffi::TerminalOption::USERDATA,
                ctx_ptr,
            );
            ffi::ghostty_terminal_set(
                raw.as_ptr(),
                ffi::TerminalOption::WRITE_PTY,
                pty_write_callback as *const c_void,
            );
        }

        let pty = Pty::spawn(seance_pty::Size { cols, rows }).ok()?;
        Some(Self {
            raw,
            ctx,
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
                    unsafe {
                        ffi::ghostty_terminal_vt_write(self.raw.as_ptr(), buf.as_ptr(), n);
                    }
                    got_data = true;
                }
                _ => break,
            }
        }
        if !self.ctx.response_buf.is_empty() {
            let responses: Vec<u8> = std::mem::take(&mut self.ctx.response_buf);
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
        unsafe { ffi::ghostty_terminal_resize(self.raw.as_ptr(), cols, rows, 0, 0) };
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    pub fn resize_pty(&mut self, cols: u16, rows: u16) {
        let _ = self.pty.resize(seance_pty::Size { cols, rows });
    }

    pub fn resize_vt(&mut self, cols: u16, rows: u16) {
        unsafe { ffi::ghostty_terminal_resize(self.raw.as_ptr(), cols, rows, 0, 0) };
    }

    pub fn size(&self) -> (u16, u16) {
        let mut cols: u16 = 0;
        let mut rows: u16 = 0;
        unsafe {
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::COLS, (&mut cols as *mut u16).cast());
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::ROWS, (&mut rows as *mut u16).cast());
        }
        (cols, rows)
    }

    pub fn cursor(&self) -> CursorState {
        let mut col: u16 = 0;
        let mut row: u16 = 0;
        let mut visible: bool = true;
        unsafe {
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::CURSOR_X, (&mut col as *mut u16).cast());
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::CURSOR_Y, (&mut row as *mut u16).cast());
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::CURSOR_VISIBLE, (&mut visible as *mut bool).cast());
        }
        CursorState { col, row, visible }
    }

    pub fn title(&self) -> Option<String> {
        let mut s = ffi::String { ptr: std::ptr::null(), len: 0 };
        let result = unsafe {
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::TITLE, (&mut s as *mut ffi::String).cast())
        };
        if result != ffi::Result::SUCCESS || s.len == 0 || s.ptr.is_null() {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(s.ptr, s.len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    pub fn scroll(&mut self, action: ScrollAction) {
        let sv = match action {
            ScrollAction::Lines(delta) => {
                let mut v = ffi::TerminalScrollViewport::default();
                v.tag = ffi::TerminalScrollViewportTag::DELTA;
                v.value.delta = delta as isize;
                v
            }
            ScrollAction::Top => {
                let mut v = ffi::TerminalScrollViewport::default();
                v.tag = ffi::TerminalScrollViewportTag::TOP;
                v
            }
            ScrollAction::Bottom => {
                let mut v = ffi::TerminalScrollViewport::default();
                v.tag = ffi::TerminalScrollViewportTag::BOTTOM;
                v
            }
            ScrollAction::PageUp => {
                let (_, rows) = self.size();
                let mut v = ffi::TerminalScrollViewport::default();
                v.tag = ffi::TerminalScrollViewportTag::DELTA;
                v.value.delta = -(rows as isize);
                v
            }
            ScrollAction::PageDown => {
                let (_, rows) = self.size();
                let mut v = ffi::TerminalScrollViewport::default();
                v.tag = ffi::TerminalScrollViewportTag::DELTA;
                v.value.delta = rows as isize;
                v
            }
        };
        unsafe { ffi::ghostty_terminal_scroll_viewport(self.raw.as_ptr(), sv) };
    }

    pub fn scroll_to_top(&mut self) { self.scroll(ScrollAction::Top); }
    pub fn scroll_to_bottom(&mut self) { self.scroll(ScrollAction::Bottom); }

    pub fn scrollback_rows(&self) -> usize {
        let mut rows: usize = 0;
        unsafe {
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::SCROLLBACK_ROWS, (&mut rows as *mut usize).cast());
        }
        rows
    }

    fn mode_get(&self, mode: ffi::Mode) -> bool {
        let mut value = false;
        unsafe { ffi::ghostty_terminal_mode_get(self.raw.as_ptr(), mode, &mut value) };
        value
    }

    pub fn modes(&self) -> TerminalModes {
        let mut mouse_tracking = false;
        unsafe {
            ffi::ghostty_terminal_get(self.raw.as_ptr(), ffi::TerminalData::MOUSE_TRACKING, (&mut mouse_tracking as *mut bool).cast());
        }
        TerminalModes {
            cursor_keys: self.mode_get(MODE_DECCKM),
            mouse_event: if mouse_tracking { 1000 } else { 0 },
            mouse_format_sgr: self.mode_get(MODE_SGR_MOUSE),
            synchronized_output: self.mode_get(MODE_SYNC_OUTPUT),
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

    /// The raw GhosttyTerminal handle for passing to the renderer.
    pub(crate) fn raw_terminal_ptr(&self) -> *mut c_void {
        self.raw.as_ptr().cast()
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        unsafe { ffi::ghostty_terminal_free(self.raw.as_ptr()) };
    }
}
