use std::collections::VecDeque;
use std::fmt;
use std::sync::mpsc;

use bytes::Bytes;
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{Mode, ScrollViewport};
use libghostty_vt::{RenderState, Terminal as VtTerminal, TerminalOptions};

use crate::frame::{CursorShape, DirtySnapshot};
use crate::frame_source::{SnapshotExtraction, extract_snapshot};
use crate::snapshot::VtSnapshot;
use crate::terminal::install_png_decoder_for_this_thread;

pub(crate) const DEFAULT_MAX_SCROLLBACK: usize = 10_000;
pub(crate) const KITTY_IMAGE_STORAGE_LIMIT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VtCoreError {
    LibGhostty(&'static str),
}

impl VtCoreError {
    pub(crate) const fn libghostty(op: &'static str) -> Self {
        Self::LibGhostty(op)
    }
}

impl fmt::Display for VtCoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LibGhostty(op) => write!(f, "libghostty operation failed: {op}"),
        }
    }
}

impl std::error::Error for VtCoreError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VtCoreOptions {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
    pub max_scrollback: usize,
    pub initial_cursor_shape: CursorShape,
}

pub(crate) struct VtCore {
    vt: Box<VtTerminal<'static, 'static>>,
    render_state: RenderState<'static>,
    response_rx: mpsc::Receiver<Bytes>,
    cell_width_px: u32,
    cell_height_px: u32,
    dirty: DirtyTracker,
    row_cache: Option<RowCache>,
    force_full_next_snapshot: bool,
}

impl VtCore {
    pub(crate) fn new(options: VtCoreOptions) -> Result<Self, VtCoreError> {
        install_png_decoder_for_this_thread();

        let mut vt = Box::new(
            VtTerminal::new(TerminalOptions {
                cols: options.cols,
                rows: options.rows,
                max_scrollback: options.max_scrollback,
            })
            .map_err(|_| VtCoreError::libghostty("terminal new"))?,
        );
        configure_kitty_graphics(&mut vt)?;

        let render_state =
            RenderState::new().map_err(|_| VtCoreError::libghostty("render state new"))?;

        let (cell_width_px, cell_height_px) = cell_px(
            options.cols,
            options.rows,
            options.pixel_width,
            options.pixel_height,
        );
        vt.resize(options.cols, options.rows, cell_width_px, cell_height_px)
            .map_err(|_| VtCoreError::libghostty("terminal resize"))?;

        let (response_tx, response_rx) = mpsc::channel::<Bytes>();
        vt.on_pty_write(move |_, data| {
            let _ = response_tx.send(Bytes::copy_from_slice(data));
        })
        .map_err(|_| VtCoreError::libghostty("on pty write"))?;

        let mut core = Self {
            vt,
            render_state,
            response_rx,
            cell_width_px,
            cell_height_px,
            dirty: DirtyTracker::default(),
            row_cache: None,
            force_full_next_snapshot: false,
        };
        core.seed_cursor_shape(options.initial_cursor_shape);
        Ok(core)
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            self.vt.vt_write(bytes);
        }
    }

    pub(crate) fn drain_responses(&mut self) -> Vec<Bytes> {
        let mut out = Vec::new();
        while let Ok(bytes) = self.response_rx.try_recv() {
            out.push(bytes);
        }
        out
    }

    pub(crate) fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), VtCoreError> {
        let (cell_w, cell_h) = cell_px(cols, rows, pixel_width, pixel_height);
        self.cell_width_px = cell_w;
        self.cell_height_px = cell_h;
        self.force_full_next_snapshot = true;
        self.vt
            .resize(cols, rows, cell_w, cell_h)
            .map_err(|_| VtCoreError::libghostty("terminal resize"))
    }

    pub(crate) fn set_theme_colors(
        &mut self,
        fg: [u8; 3],
        bg: [u8; 3],
        cursor: [u8; 3],
        palette: [[u8; 3]; 256],
    ) {
        let rgb = |[r, g, b]: [u8; 3]| RgbColor { r, g, b };
        self.force_full_next_snapshot = true;
        let _ = self.vt.set_default_fg_color(Some(rgb(fg)));
        let _ = self.vt.set_default_bg_color(Some(rgb(bg)));
        let _ = self.vt.set_default_cursor_color(Some(rgb(cursor)));
        let _ = self.vt.set_default_color_palette(Some(palette.map(rgb)));
    }

    pub(crate) fn seed_cursor_shape(&mut self, shape: CursorShape) {
        self.vt.vt_write(cursor_shape_sequence(shape));
    }

    pub(crate) fn scroll_lines(&mut self, delta: i32) {
        self.vt
            .scroll_viewport(ScrollViewport::Delta(delta as isize));
    }

    pub(crate) fn snapshot(&mut self) -> Result<VtSnapshot, VtCoreError> {
        let SnapshotExtraction {
            mut snapshot,
            dirty_delta,
        } = extract_snapshot(
            &mut self.vt,
            &mut self.render_state,
            self.cell_width_px,
            self.cell_height_px,
        )?;
        let dirty_delta = self.snapshot_dirty_delta(&snapshot, dirty_delta);
        let generation = self.dirty.next_generation();
        let dirty = self.dirty.record(generation, dirty_delta);
        snapshot.generation = generation;
        snapshot.dirty = dirty;
        Ok(snapshot)
    }

    pub(crate) fn ack_rendered(&mut self, generation: u64) {
        self.dirty.ack_rendered(generation);
    }

    fn snapshot_dirty_delta(
        &mut self,
        snapshot: &VtSnapshot,
        libghostty_delta: DirtySnapshot,
    ) -> DirtySnapshot {
        let current = RowCache::from_snapshot(snapshot);
        let row_delta = self
            .row_cache
            .as_ref()
            .map_or(DirtySnapshot::Full, |previous| previous.diff(&current));
        self.row_cache = Some(current);

        if self.force_full_next_snapshot || matches!(libghostty_delta, DirtySnapshot::Full) {
            self.force_full_next_snapshot = false;
            DirtySnapshot::Full
        } else {
            row_delta
        }
    }

    pub(crate) fn sync_active(&self) -> bool {
        self.vt.mode(Mode::SYNC_OUTPUT).unwrap_or(false)
    }

    pub(crate) fn cols(&self) -> u16 {
        self.vt.cols().unwrap_or(0)
    }

    pub(crate) fn rows(&self) -> u16 {
        self.vt.rows().unwrap_or(0)
    }

    pub(crate) fn cursor_pos(&self) -> (u16, u16) {
        (
            self.vt.cursor_x().unwrap_or(0),
            self.vt.cursor_y().unwrap_or(0),
        )
    }

    pub(crate) fn is_cursor_visible(&self) -> bool {
        self.vt.is_cursor_visible().unwrap_or(true)
    }

    pub(crate) fn mode(&self, mode: Mode) -> bool {
        self.vt.mode(mode).unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowCache {
    cols: u16,
    rows: u16,
    rows_data: Vec<Vec<RowCell>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowCell {
    text: String,
    fg: crate::frame::CellColor,
    bg: crate::frame::CellColor,
    attrs: crate::frame::CellAttrs,
}

impl RowCache {
    fn from_snapshot(snapshot: &VtSnapshot) -> Self {
        let mut rows_data = Vec::with_capacity(usize::from(snapshot.rows));
        for row in 0..snapshot.rows {
            let mut row_data = Vec::with_capacity(usize::from(snapshot.cols));
            for col in 0..snapshot.cols {
                let idx = usize::from(row) * usize::from(snapshot.cols) + usize::from(col);
                let Some(cell) = snapshot.cells.get(idx) else {
                    continue;
                };
                row_data.push(RowCell {
                    text: snapshot.cell_text(cell).to_owned(),
                    fg: cell.fg,
                    bg: cell.bg,
                    attrs: cell.attrs,
                });
            }
            rows_data.push(row_data);
        }
        Self {
            cols: snapshot.cols,
            rows: snapshot.rows,
            rows_data,
        }
    }

    fn diff(&self, current: &Self) -> DirtySnapshot {
        if self.cols != current.cols || self.rows != current.rows {
            return DirtySnapshot::Full;
        }
        let rows = self
            .rows_data
            .iter()
            .zip(&current.rows_data)
            .enumerate()
            .filter_map(|(idx, (previous, current))| {
                if previous == current {
                    None
                } else {
                    u16::try_from(idx).ok()
                }
            })
            .collect::<Vec<_>>();
        if rows.is_empty() {
            DirtySnapshot::Clean
        } else {
            DirtySnapshot::Partial(rows)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirtyDelta {
    generation: u64,
    dirty: DirtySnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirtyTracker {
    next_generation: u64,
    first_snapshot: bool,
    unacknowledged: VecDeque<DirtyDelta>,
}

impl Default for DirtyTracker {
    fn default() -> Self {
        Self {
            next_generation: 1,
            first_snapshot: true,
            unacknowledged: VecDeque::new(),
        }
    }
}

impl DirtyTracker {
    fn next_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        generation
    }

    fn record(&mut self, generation: u64, dirty: DirtySnapshot) -> DirtySnapshot {
        let dirty = if self.first_snapshot {
            self.first_snapshot = false;
            DirtySnapshot::Full
        } else {
            normalize_dirty(dirty)
        };

        if !matches!(dirty, DirtySnapshot::Clean) {
            self.unacknowledged
                .push_back(DirtyDelta { generation, dirty });
        }
        union_dirty(self.unacknowledged.iter().map(|delta| &delta.dirty))
    }

    fn ack_rendered(&mut self, generation: u64) {
        while self
            .unacknowledged
            .front()
            .is_some_and(|delta| delta.generation <= generation)
        {
            self.unacknowledged.pop_front();
        }
    }
}

fn normalize_dirty(dirty: DirtySnapshot) -> DirtySnapshot {
    match dirty {
        DirtySnapshot::Partial(mut rows) => {
            rows.sort_unstable();
            rows.dedup();
            if rows.is_empty() {
                DirtySnapshot::Clean
            } else {
                DirtySnapshot::Partial(rows)
            }
        }
        other => other,
    }
}

fn union_dirty<'a>(dirty: impl IntoIterator<Item = &'a DirtySnapshot>) -> DirtySnapshot {
    let mut rows = Vec::new();
    for dirty in dirty {
        match dirty {
            DirtySnapshot::Full => return DirtySnapshot::Full,
            DirtySnapshot::Clean => {}
            DirtySnapshot::Partial(partial) => rows.extend(partial.iter().copied()),
        }
    }
    if rows.is_empty() {
        DirtySnapshot::Clean
    } else {
        rows.sort_unstable();
        rows.dedup();
        DirtySnapshot::Partial(rows)
    }
}

fn configure_kitty_graphics(vt: &mut Box<VtTerminal<'static, 'static>>) -> Result<(), VtCoreError> {
    vt.set_kitty_image_storage_limit(KITTY_IMAGE_STORAGE_LIMIT_BYTES)
        .map_err(|_| VtCoreError::libghostty("kitty storage limit"))?;
    let _ = vt.set_kitty_image_from_file_allowed(true);
    let _ = vt.set_kitty_image_from_temp_file_allowed(true);
    let _ = vt.set_kitty_image_from_shared_mem_allowed(true);
    Ok(())
}

pub(crate) fn cell_px(cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> (u32, u32) {
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

pub(crate) fn cursor_shape_sequence(shape: CursorShape) -> &'static [u8] {
    match shape {
        CursorShape::Block => b"\x1b[2 q",
        CursorShape::Bar => b"\x1b[6 q",
        CursorShape::Underline => b"\x1b[4 q",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> VtCore {
        VtCore::new(VtCoreOptions {
            cols: 4,
            rows: 3,
            pixel_width: 40,
            pixel_height: 30,
            max_scrollback: DEFAULT_MAX_SCROLLBACK,
            initial_cursor_shape: CursorShape::Block,
        })
        .expect("core should construct")
    }

    #[test]
    fn generations_increase() {
        let mut core = core();
        let first = core.snapshot().unwrap();
        let second = core.snapshot().unwrap();
        assert!(second.generation > first.generation);
    }

    #[test]
    fn initial_snapshot_is_full() {
        let mut core = core();
        let snapshot = core.snapshot().unwrap();
        assert_eq!(snapshot.dirty, DirtySnapshot::Full);
    }

    #[test]
    fn no_ack_preserves_dirty_across_snapshots() {
        let mut core = core();
        let first = core.snapshot().unwrap();
        let second = core.snapshot().unwrap();
        assert_eq!(first.dirty, DirtySnapshot::Full);
        assert_eq!(second.dirty, DirtySnapshot::Full);
    }

    #[test]
    fn ack_latest_clears_dirty() {
        let mut core = core();
        let first = core.snapshot().unwrap();
        core.ack_rendered(first.generation);
        let second = core.snapshot().unwrap();
        assert_eq!(second.dirty, DirtySnapshot::Clean);
    }

    #[test]
    fn row_dirty_is_partial_after_initial_ack() {
        let mut core = core();
        let initial = core.snapshot().unwrap();
        core.ack_rendered(initial.generation);

        core.feed(b"x");
        let snapshot = core.snapshot().unwrap();
        assert_eq!(snapshot.dirty, DirtySnapshot::Partial(vec![0]));
    }

    #[test]
    fn stale_ack_does_not_clear_newer_dirty() {
        let mut core = core();
        let initial = core.snapshot().unwrap();
        core.ack_rendered(initial.generation);

        core.feed(b"a");
        let stale = core.snapshot().unwrap();
        core.feed(b"b");
        let newer = core.snapshot().unwrap();
        core.ack_rendered(stale.generation);

        let next = core.snapshot().unwrap();
        assert!(newer.generation > stale.generation);
        assert_ne!(next.dirty, DirtySnapshot::Clean);
    }

    #[test]
    fn full_persists_until_acked() {
        let mut core = core();
        let initial = core.snapshot().unwrap();
        core.ack_rendered(initial.generation);

        core.resize(6, 4, 60, 40).unwrap();
        let resized = core.snapshot().unwrap();
        assert_eq!(resized.dirty, DirtySnapshot::Full);

        let still_full = core.snapshot().unwrap();
        assert_eq!(still_full.dirty, DirtySnapshot::Full);

        core.ack_rendered(still_full.generation);
        let clean = core.snapshot().unwrap();
        assert_eq!(clean.dirty, DirtySnapshot::Clean);
    }
}
