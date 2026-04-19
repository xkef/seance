//! High-level terminal: VT emulator + PTY + selection.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::Once;

use libghostty_vt::alloc::{Allocator, Bytes};
use libghostty_vt::kitty::graphics::{self, DecodePng, DecodedImage};
use libghostty_vt::render::{CellIterator, RowIterator};
use libghostty_vt::terminal::{Mode, ScrollViewport};
use libghostty_vt::{RenderState, Terminal as VtTerminal, TerminalOptions};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::modes::TerminalModes;
use crate::selection::{GridPos, Selection, SelectionGranularity};

const READ_CHUNK: usize = 4096;
const MAX_SCROLLBACK: usize = 10_000;

/// Kitty image storage cap per terminal. Non-zero enables the protocol;
/// ghostty evicts oldest images when we would exceed this limit.
const KITTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

/// PNG decoder for the Kitty graphics protocol.
///
/// Ghostty's storage accepts RGBA8 pixels, so palette and grayscale
/// inputs are expanded; 16-bit depths are stripped to 8-bit.
struct PngDecoder;

impl DecodePng for PngDecoder {
    fn decode_png<'alloc>(
        &mut self,
        alloc: &'alloc Allocator<'_>,
        data: &[u8],
    ) -> Option<DecodedImage<'alloc>> {
        use png::{Decoder, Transformations};

        let mut decoder = Decoder::new(std::io::Cursor::new(data));
        decoder.set_transformations(Transformations::ALPHA | Transformations::STRIP_16);

        let mut reader = decoder.read_info().ok()?;
        let buf_size = reader.output_buffer_size()?;
        let mut scratch = vec![0u8; buf_size];
        let info = reader.next_frame(&mut scratch).ok()?;

        let mut bytes = Bytes::new_with_alloc(alloc, info.buffer_size()).ok()?;
        bytes.copy_from_slice(&scratch[..info.buffer_size()]);

        Some(DecodedImage {
            width: info.width,
            height: info.height,
            data: bytes,
        })
    }
}

/// Install the PNG decoder. Safe to call repeatedly; only takes effect once.
/// The decoder is thread-local inside libghostty-vt, so this must run on the
/// thread that owns the terminal.
fn install_png_decoder_once() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let _ = graphics::set_png_decoder(Some(PngDecoder));
    });
}

/// A terminal session: VT emulator, PTY, and text selection state.
pub struct Terminal {
    vt: Box<VtTerminal<'static, 'static>>,
    response_buf: Rc<RefCell<Vec<u8>>>,
    reader: Box<dyn Read + Send>,
    writer: RefCell<Box<dyn Write + Send>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    selection: Option<Selection>,
    /// Per-cell pixel dimensions as last passed to `vt.resize()`. Needed
    /// to convert virtual-placement cell coordinates into pixel rects.
    cell_width_px: u32,
    cell_height_px: u32,
}

impl Terminal {
    /// Spawn a new shell in a PTY.
    pub fn spawn(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Option<Self> {
        install_png_decoder_once();

        let mut vt = Box::new(
            VtTerminal::new(TerminalOptions {
                cols,
                rows,
                max_scrollback: MAX_SCROLLBACK,
            })
            .ok()?,
        );

        // Enable Kitty graphics storage. Zero disables it entirely;
        // ghostty needs a non-zero cap and cell pixel dimensions to
        // compute placement grid sizes (set via resize below).
        vt.set_kitty_image_storage_limit(KITTY_IMAGE_STORAGE_LIMIT_BYTES)
            .ok()?;
        // Allow file / temp-file / shared-mem transmission mediums.
        // Yazi and similar previewers default to `t=t` (temp file) for
        // anything over a few KB; disabling these makes the transmit
        // silently fail, leaving no image in storage to render.
        let _ = vt.set_kitty_image_from_file_allowed(true);
        let _ = vt.set_kitty_image_from_temp_file_allowed(true);
        let _ = vt.set_kitty_image_from_shared_mem_allowed(true);
        let (cell_w, cell_h) = cell_px(cols, rows, pixel_width, pixel_height);
        vt.resize(cols, rows, cell_w, cell_h).ok()?;

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
            cell_width_px: cell_w,
            cell_height_px: cell_h,
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
        let (cell_w, cell_h) = cell_px(cols, rows, pixel_width, pixel_height);
        self.cell_width_px = cell_w;
        self.cell_height_px = cell_h;
        let _ = self.vt.resize(cols, rows, cell_w, cell_h);
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

    pub(crate) fn vt(&self) -> &VtTerminal<'static, 'static> {
        &self.vt
    }

    /// Cell pixel dimensions as last passed to `vt.resize()`. Both are
    /// `0` until the first resize, in which case virtual placements
    /// can't be rendered (no way to size them).
    pub(crate) fn cell_pixels(&self) -> (u32, u32) {
        (self.cell_width_px, self.cell_height_px)
    }
}

/// Derive cell pixel dimensions from a viewport's total pixel size.
/// Returns (0, 0) when either axis has no cells or no pixels — libghostty-vt
/// handles zero dimensions by disabling pixel-sensitive features for that axis.
fn cell_px(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> (u32, u32) {
    let w = if cols == 0 {
        0
    } else {
        u32::from(pixel_width) / u32::from(cols)
    };
    let h = if rows == 0 {
        0
    } else {
        u32::from(pixel_height) / u32::from(rows)
    };
    (w, h)
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
