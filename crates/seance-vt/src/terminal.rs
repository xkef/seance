//! High-level terminal: VT emulator + PTY.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use libghostty_vt::alloc::{Allocator, Bytes};
use libghostty_vt::kitty::graphics::{self, DecodePng, DecodedImage};
use libghostty_vt::render::{CellIterator, Dirty, RowIterator};
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{Mode, ScrollViewport};
use libghostty_vt::{RenderState, Terminal as VtTerminal, TerminalOptions};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::frame::DirtySnapshot;
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

/// Install the PNG decoder for the current thread.
///
/// The decoder is thread-local inside libghostty-vt, so this must run on the
/// thread that owns the terminal.
pub fn install_png_decoder_for_this_thread() {
    let _ = graphics::set_png_decoder(Some(Box::new(PngDecoder)));
}

/// A terminal session: VT emulator and PTY.
pub struct Terminal {
    vt: Box<VtTerminal<'static, 'static>>,
    /// Persistent render state. libghostty-vt's dirty tracking only
    /// works when the same render state is reused across frames — each
    /// `update(vt)` drains the VT's dirty set into the render state, and
    /// per-row flags remain until the caller clears them.
    render_state: RenderState<'static>,
    response_buf: Arc<Mutex<Vec<u8>>>,
    /// `None` after [`Self::take_reader`]: the reader has been moved off
    /// to a dedicated thread that does blocking PTY reads.
    reader: Option<Box<dyn Read + Send>>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// Per-cell pixel dimensions as last passed to `vt.resize()`. Needed
    /// to convert virtual-placement cell coordinates into pixel rects.
    cell_width_px: u32,
    cell_height_px: u32,
}

impl Terminal {
    /// Spawn a new shell in a PTY.
    pub fn spawn(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Option<Self> {
        install_png_decoder_for_this_thread();

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

        let response_buf = Arc::new(Mutex::new(Vec::new()));
        let buf = Arc::clone(&response_buf);
        vt.on_pty_write(move |_, data| {
            buf.lock()
                .expect("terminal response buffer mutex poisoned")
                .extend_from_slice(data);
        })
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

        let render_state = RenderState::new().ok()?;

        Some(Self {
            vt,
            render_state,
            response_buf,
            reader: Some(reader),
            writer,
            master: pair.master,
            child,
            cell_width_px: cell_w,
            cell_height_px: cell_h,
        })
    }

    /// Move the PTY reader out of the terminal so a dedicated IO thread
    /// can own the blocking `read()`. Returns `None` if a previous caller
    /// already took it. After this, [`Self::feed`] is the only path that
    /// drives PTY bytes into the VT.
    pub fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    /// Suggested chunk size for the IO thread reading PTY output.
    pub const READ_CHUNK_SIZE: usize = READ_CHUNK;

    /// Feed PTY output into the VT and flush any device responses the VT
    /// produced (cursor reports, DA1, etc.) back to the shell. Replaces
    /// the old `poll()` path; the read itself happens on the IO thread.
    pub fn feed(&mut self, data: &[u8]) {
        if !data.is_empty() {
            self.vt.vt_write(data);
        }
        let responses = {
            let mut buf = self
                .response_buf
                .lock()
                .expect("terminal response buffer mutex poisoned");
            std::mem::take(&mut *buf)
        };
        if !responses.is_empty() {
            let _ = self.writer.write_all(&responses);
        }
    }

    /// Seed the VT's cursor shape with a DECSCUSR sequence (steady
    /// variants). Feeds bytes directly into the VT stream so the VT
    /// state matches the caller's preferred default at frame 0;
    /// DECSCUSR emissions from shells / editors still override on
    /// subsequent frames. Does not touch the PTY.
    pub fn set_cursor_shape(&mut self, shape: crate::frame::CursorShape) {
        let seq: &[u8] = match shape {
            crate::frame::CursorShape::Block => b"\x1b[2 q",
            crate::frame::CursorShape::Bar => b"\x1b[6 q",
            crate::frame::CursorShape::Underline => b"\x1b[4 q",
        };
        self.vt.vt_write(seq);
    }

    /// Write raw bytes to the PTY (keyboard input, paste, etc.).
    pub fn write(&mut self, data: &[u8]) {
        let _ = self.writer.write_all(data);
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

    pub fn set_theme_colors(
        &mut self,
        fg: [u8; 3],
        bg: [u8; 3],
        cursor: [u8; 3],
        palette: [[u8; 3]; 256],
    ) {
        let rgb = |[r, g, b]: [u8; 3]| RgbColor { r, g, b };
        let _ = self.vt.set_default_fg_color(Some(rgb(fg)));
        let _ = self.vt.set_default_bg_color(Some(rgb(bg)));
        let _ = self.vt.set_default_cursor_color(Some(rgb(cursor)));
        let _ = self.vt.set_default_color_palette(Some(palette.map(rgb)));
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

    /// Extract the selected text from the live VT grid.
    pub fn selection_text(&mut self, sel: &Selection) -> Option<String> {
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

    /// Drain the VT's dirty state into the persistent render state and
    /// report what changed. The flags remain set on the render state (and
    /// can be re-read) until [`Self::clear_dirty`] acknowledges them.
    ///
    /// Returns `None` only on FFI failure — callers should treat that as
    /// "unknown, redraw everything" for safety.
    pub(crate) fn dirty_snapshot(&mut self) -> Option<DirtySnapshot> {
        let snapshot = self.render_state.update(&self.vt).ok()?;
        let global = snapshot.dirty().ok()?;
        match global {
            Dirty::Clean => Some(DirtySnapshot::Clean),
            Dirty::Full => Some(DirtySnapshot::Full),
            Dirty::Partial => {
                let mut rows_iter = RowIterator::new().ok()?;
                let mut iter = rows_iter.update(&snapshot).ok()?;
                let mut out: Vec<u16> = Vec::new();
                let mut idx: u16 = 0;
                while let Some(row) = iter.next() {
                    if row.dirty().unwrap_or(true) {
                        out.push(idx);
                    }
                    idx += 1;
                }
                // Global "Partial" with no dirty rows can arise when only
                // cursor / selection / cursor-color changed. Fold to Clean
                // so callers can short-circuit the whole rebuild.
                if out.is_empty() {
                    Some(DirtySnapshot::Clean)
                } else {
                    Some(DirtySnapshot::Partial(out))
                }
            }
        }
    }

    /// Clear both the global frame-dirty signal and every per-row flag on
    /// the persistent render state. Call after consuming a dirty snapshot.
    pub(crate) fn clear_dirty(&mut self) {
        let Ok(snapshot) = self.render_state.update(&self.vt) else {
            return;
        };
        let _ = snapshot.set_dirty(Dirty::Clean);
        let Ok(mut rows_iter) = RowIterator::new() else {
            return;
        };
        let Ok(mut iter) = rows_iter.update(&snapshot) else {
            return;
        };
        while let Some(row) = iter.next() {
            let _ = row.set_dirty(false);
        }
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

#[cfg(test)]
mod tests {
    use libghostty_vt::RenderState;
    use libghostty_vt::Terminal as VtTerminal;

    use super::*;

    #[test]
    fn terminal_is_send() {
        fn assert_send<T: Send>() {}

        assert_send::<Terminal>();
        assert_send::<VtTerminal<'static, 'static>>();
        assert_send::<RenderState<'static>>();
    }

    #[test]
    fn terminal_can_be_constructed_on_worker_thread() {
        std::thread::spawn(|| {
            install_png_decoder_for_this_thread();
            let mut terminal =
                Terminal::spawn(24, 5, 240, 80).expect("worker thread should construct terminal");
            terminal.feed(b"\x1b[c");
        })
        .join()
        .expect("worker thread should not panic");
    }
}
