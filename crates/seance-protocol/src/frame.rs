use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

use crate::identity::ImageId;

/// Sentinel for [`SnapshotCell::hyperlink_idx`] meaning "this cell has no
/// OSC 8 hyperlink." Real indices reference [`VtSnapshot::hyperlinks`].
pub const NO_HYPERLINK: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start: i64,
    pub count: u16,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridPos {
    pub col: u16,
    pub row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionGranularity {
    Character,
    Word,
    Line,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    anchor: GridPos,
    head: GridPos,
    granularity: SelectionGranularity,
}

impl Selection {
    pub fn new(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Character)
    }

    pub fn new_line(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Line)
    }

    /// Word selection spanning a precomputed `(start, end)` pair. Both
    /// endpoints must already point at the inclusive boundaries of the
    /// word; rendering uses them verbatim.
    pub fn word_range(start: GridPos, end: GridPos) -> Self {
        Self {
            anchor: start,
            head: end,
            granularity: SelectionGranularity::Word,
        }
    }

    /// Line selection spanning a precomputed `(start, end)` pair, used
    /// for wrap-aware logical lines.
    pub fn line_range(start: GridPos, end: GridPos) -> Self {
        Self {
            anchor: start,
            head: end,
            granularity: SelectionGranularity::Line,
        }
    }

    fn at(pos: GridPos, granularity: SelectionGranularity) -> Self {
        Self {
            anchor: pos,
            head: pos,
            granularity,
        }
    }

    pub fn update(&mut self, pos: GridPos) {
        self.head = pos;
    }

    pub fn granularity(&self) -> SelectionGranularity {
        self.granularity
    }

    pub fn ordered_range(&self) -> (GridPos, GridPos) {
        let (a, b) = (self.anchor, self.head);
        if (a.row, a.col) <= (b.row, b.col) {
            (a, b)
        } else {
            (b, a)
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_tracking: MouseTracking,
    pub mouse_format_sgr: bool,
    pub bracketed_paste: bool,
    pub alt_screen: bool,
    /// DECSET 1007: when set and `alt_screen` is true, the host translates
    /// wheel events into Up/Down arrow sequences instead of touching the
    /// (absent) alt-screen scrollback.
    pub alt_scroll: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseTracking {
    #[default]
    None,
    X10,
    Normal,
    Button,
    Any,
}

impl MouseTracking {
    pub fn is_enabled(self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn reports_motion(self) -> bool {
        matches!(self, Self::Button | Self::Any)
    }

    pub fn reports_motion_without_button(self) -> bool {
        matches!(self, Self::Any)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseSize {
    pub screen_width: u32,
    pub screen_height: u32,
    pub cell_width: u32,
    pub cell_height: u32,
    pub padding_top: u32,
    pub padding_bottom: u32,
    pub padding_left: u32,
    pub padding_right: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CellColor {
    Default,
    Palette(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellAttrs {
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub inverse: bool,
    pub invisible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorInfo {
    pub pos: GridPos,
    pub visible: bool,
    pub wide: bool,
    pub shape: Option<CursorShape>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DirtySnapshot {
    Clean,
    Partial(Vec<u16>),
    Full,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowMeta {
    pub wrap: bool,
    pub wrap_continuation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementSnapshot {
    pub image_id: ImageId,
    pub placement_id: u32,
    pub viewport_col: i32,
    pub viewport_row: i32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    pub image_width: u32,
    pub image_height: u32,
    pub z: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VtSnapshot {
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<SnapshotCell>,
    pub text: String,
    pub rows_meta: Vec<RowMeta>,
    pub pwd: Option<String>,
    pub cursor: CursorInfo,
    pub modes: TerminalModes,
    pub dirty: DirtySnapshot,
    pub placements: Vec<PlacementSnapshot>,
    pub images: Vec<SnapshotImage>,
    /// OSC 8 hyperlink URL table. Cells reference entries by index via
    /// [`SnapshotCell::hyperlink_idx`]; [`NO_HYPERLINK`] means the cell
    /// has no hyperlink.
    pub hyperlinks: Vec<String>,
}

impl VtSnapshot {
    pub fn empty(cols: u16, rows: u16) -> Self {
        Self {
            generation: 0,
            cols,
            rows,
            cells: Vec::with_capacity(usize::from(cols) * usize::from(rows)),
            text: String::new(),
            rows_meta: vec![RowMeta::default(); usize::from(rows)],
            pwd: None,
            cursor: CursorInfo::default(),
            modes: TerminalModes::default(),
            dirty: DirtySnapshot::Full,
            placements: Vec::new(),
            images: Vec::new(),
            hyperlinks: Vec::new(),
        }
    }

    /// Inclusive `(start, end)` range of the word containing `(row, col)`.
    /// Returns `None` if the cell is empty/whitespace, or out of bounds.
    /// A "word" extends across consecutive cells whose text contains at
    /// least one word character (alphanumeric, `_`, `-`, `.`, `/`, `:`) —
    /// covers identifiers, paths, and URLs while breaking on whitespace
    /// and most punctuation.
    pub fn word_range_at(&self, row: u16, col: u16) -> Option<(GridPos, GridPos)> {
        if row >= self.rows || col >= self.cols {
            return None;
        }
        if !self.cell_is_word(row, col) {
            return None;
        }
        let mut start = col;
        while start > 0 && self.cell_is_word(row, start - 1) {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < self.cols && self.cell_is_word(row, end + 1) {
            end += 1;
        }
        Some((GridPos { col: start, row }, GridPos { col: end, row }))
    }

    fn cell_is_word(&self, row: u16, col: u16) -> bool {
        let Some(cell) = self.cell_at(row, col) else {
            return false;
        };
        let text = self.cell_text(cell);
        text.chars().any(is_word_char)
    }

    /// Inclusive row range of the wrap chain containing `row` — i.e. the
    /// rows that together form one logical (possibly soft-wrapped) line.
    /// Walks up while the previous row is a wrap continuation, then down
    /// while the current row's `wrap` flag is set.
    pub fn wrap_chain(&self, row: u16) -> (u16, u16) {
        if row >= self.rows || self.rows == 0 {
            return (row, row);
        }
        let mut start = row;
        while start > 0
            && self
                .rows_meta
                .get(usize::from(start))
                .is_some_and(|m| m.wrap_continuation)
        {
            start -= 1;
        }
        let mut end = row;
        while end + 1 < self.rows && self.rows_meta.get(usize::from(end)).is_some_and(|m| m.wrap) {
            end += 1;
        }
        (start, end)
    }

    pub fn cell_text(&self, cell: &SnapshotCell) -> &str {
        let start = cell.text_start as usize;
        let end = start.saturating_add(usize::from(cell.text_len));
        self.text.get(start..end).unwrap_or("")
    }

    pub fn selection_text(&self, sel: &Selection) -> Option<String> {
        let (start, end) = sel.ordered_range();
        let granularity = sel.granularity();

        let mut out = String::new();
        for row in 0..self.rows {
            if row > end.row {
                break;
            }
            if row < start.row {
                continue;
            }

            let (col_start, col_end) = column_range(granularity, row, start, end, self.cols);
            if !out.is_empty() {
                out.push('\n');
            }

            for col in 0..self.cols {
                if col < col_start || col > col_end {
                    continue;
                }
                let Some(cell) = self.cell_at(row, col) else {
                    continue;
                };
                let text = self.cell_text(cell);
                if text.is_empty() {
                    out.push(' ');
                } else {
                    out.push_str(text);
                }
            }

            let trimmed = out.trim_end().len();
            out.truncate(trimmed);
        }

        if out.is_empty() { None } else { Some(out) }
    }

    pub fn push_cell(&mut self, text: &str, fg: CellColor, bg: CellColor, attrs: CellAttrs) {
        let text_start = self.text.len();
        self.text.push_str(text);
        let byte_len = self.text.len() - text_start;
        let text_len = match u16::try_from(byte_len) {
            Ok(len) => len,
            Err(_) => {
                self.text.truncate(text_start);
                0
            }
        };
        self.cells.push(SnapshotCell {
            text_start: u32::try_from(text_start).unwrap_or(u32::MAX),
            text_len,
            fg,
            bg,
            attrs,
            hyperlink_idx: NO_HYPERLINK,
        });
    }

    pub fn push_empty_cell(&mut self) {
        self.cells.push(SnapshotCell::empty());
    }

    pub fn cell_at(&self, row: u16, col: u16) -> Option<&SnapshotCell> {
        let idx = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.cells.get(idx)
    }

    /// Resolve a cell's OSC 8 URL. Returns `None` when the cell carries
    /// no hyperlink or the index is out of range.
    pub fn cell_hyperlink(&self, cell: &SnapshotCell) -> Option<&str> {
        if cell.hyperlink_idx == NO_HYPERLINK {
            return None;
        }
        self.hyperlinks
            .get(usize::from(cell.hyperlink_idx))
            .map(String::as_str)
    }

    /// If the cell at `(col, row)` carries an OSC 8 hyperlink, return the
    /// contiguous run of cells on the same row that share its index along
    /// with the resolved URL.
    pub fn osc8_run_at(&self, col: u16, row: u16) -> Option<HyperlinkRun<'_>> {
        let cell = self.cell_at(row, col)?;
        if cell.hyperlink_idx == NO_HYPERLINK {
            return None;
        }
        let url = self.cell_hyperlink(cell)?;
        let target = cell.hyperlink_idx;

        let mut start = col;
        while start > 0 {
            let next = start - 1;
            match self.cell_at(row, next) {
                Some(c) if c.hyperlink_idx == target => start = next,
                _ => break,
            }
        }

        let mut end = col;
        while end + 1 < self.cols {
            let next = end + 1;
            match self.cell_at(row, next) {
                Some(c) if c.hyperlink_idx == target => end = next,
                _ => break,
            }
        }

        Some(HyperlinkRun {
            row,
            start_col: start,
            end_col: end,
            url,
        })
    }

    /// Append `url` to the hyperlink table if missing and return its index.
    /// The returned index never equals [`NO_HYPERLINK`]; if the table is
    /// already at capacity (`u16::MAX - 1` entries) this returns
    /// [`NO_HYPERLINK`] and the cell should be left unlinked.
    pub fn intern_hyperlink(&mut self, url: &str) -> u16 {
        if let Some(idx) = self.hyperlinks.iter().position(|existing| existing == url) {
            return u16::try_from(idx).unwrap_or(NO_HYPERLINK);
        }
        let idx = self.hyperlinks.len();
        if idx >= usize::from(NO_HYPERLINK) {
            return NO_HYPERLINK;
        }
        self.hyperlinks.push(url.to_owned());
        u16::try_from(idx).unwrap_or(NO_HYPERLINK)
    }

    /// Attach a hyperlink index to the most recently pushed cell. No-op if
    /// `idx == NO_HYPERLINK` or no cells have been pushed yet.
    pub fn set_last_cell_hyperlink(&mut self, idx: u16) {
        if idx == NO_HYPERLINK {
            return;
        }
        if let Some(cell) = self.cells.last_mut() {
            cell.hyperlink_idx = idx;
        }
    }

    pub fn validate_dimensions(&self) -> Result<(), FrameValidationError> {
        let expected = usize::from(self.cols) * usize::from(self.rows);
        if self.cells.len() != expected {
            return Err(FrameValidationError::InvalidCellCount {
                expected,
                actual: self.cells.len(),
            });
        }
        if self.rows_meta.len() != usize::from(self.rows) {
            return Err(FrameValidationError::InvalidRowMetaCount {
                expected: usize::from(self.rows),
                actual: self.rows_meta.len(),
            });
        }
        for cell in &self.cells {
            validate_text_range(&self.text, cell.text_start, cell.text_len)?;
        }
        Ok(())
    }
}

/// Contiguous OSC 8 run on a single row with its resolved URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyperlinkRun<'a> {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
    pub url: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotCell {
    pub text_start: u32,
    pub text_len: u16,
    pub fg: CellColor,
    pub bg: CellColor,
    pub attrs: CellAttrs,
    /// Index into [`VtSnapshot::hyperlinks`], or [`NO_HYPERLINK`] when
    /// the cell has no OSC 8 hyperlink.
    pub hyperlink_idx: u16,
}

impl SnapshotCell {
    pub fn empty() -> Self {
        Self {
            text_start: 0,
            text_len: 0,
            fg: CellColor::Default,
            bg: CellColor::Default,
            attrs: CellAttrs::default(),
            hyperlink_idx: NO_HYPERLINK,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotImage {
    pub image_id: ImageId,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resize {
    pub cols: u16,
    pub rows: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeColors {
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub cursor: [u8; 3],
    #[serde(with = "BigArray")]
    pub palette: [[u8; 3]; 256],
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FrameDelta {
    Full {
        generation: u64,
        snapshot: VtSnapshot,
    },
    Partial {
        base_generation: u64,
        generation: u64,
        cols: u16,
        rows: u16,
        cursor: CursorInfo,
        modes: TerminalModes,
        pwd: Option<String>,
        placements: Vec<PlacementSnapshot>,
        dirty_rows: Vec<RowDelta>,
        /// OSC 8 URL table for the resulting snapshot. Partial frames keep
        /// previous entries stable so unchanged base cells remain valid.
        hyperlinks: Vec<String>,
    },
}

impl FrameDelta {
    pub fn from_snapshot(previous: Option<&VtSnapshot>, snapshot: &VtSnapshot) -> Self {
        let Some(previous) = previous else {
            return Self::Full {
                generation: snapshot.generation,
                snapshot: snapshot.clone(),
            };
        };
        if previous.cols != snapshot.cols
            || previous.rows != snapshot.rows
            || previous.images != snapshot.images
            || matches!(snapshot.dirty, DirtySnapshot::Full)
            || snapshot.generation <= previous.generation
        {
            return Self::Full {
                generation: snapshot.generation,
                snapshot: snapshot.clone(),
            };
        }

        let rows = match &snapshot.dirty {
            DirtySnapshot::Clean => Vec::new(),
            DirtySnapshot::Partial(rows) => {
                let mut normalized = rows.clone();
                normalized.sort_unstable();
                normalized.dedup();
                normalized
            }
            DirtySnapshot::Full => unreachable!("full handled above"),
        };
        let mut hyperlinks = previous.hyperlinks.clone();
        let dirty_rows = rows
            .into_iter()
            .filter_map(|row| {
                RowDelta::from_snapshot_row_remapping_links(snapshot, row, &mut hyperlinks)
            })
            .collect();

        Self::Partial {
            base_generation: previous.generation,
            generation: snapshot.generation,
            cols: snapshot.cols,
            rows: snapshot.rows,
            cursor: snapshot.cursor,
            modes: snapshot.modes,
            pwd: snapshot.pwd.clone(),
            placements: snapshot.placements.clone(),
            dirty_rows,
            hyperlinks,
        }
    }

    pub fn generation(&self) -> u64 {
        match self {
            Self::Full { generation, .. } | Self::Partial { generation, .. } => *generation,
        }
    }
}

pub type WireFrame = FrameDelta;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowDelta {
    pub row: u16,
    pub meta: RowMeta,
    pub cells: Vec<SnapshotCell>,
    pub text: String,
}

impl RowDelta {
    pub fn from_snapshot_row(snapshot: &VtSnapshot, row: u16) -> Option<Self> {
        if row >= snapshot.rows {
            return None;
        }
        let mut text = String::new();
        let mut cells = Vec::with_capacity(usize::from(snapshot.cols));
        for col in 0..snapshot.cols {
            let source = *snapshot.cell_at(row, col)?;
            let cell_text = snapshot.cell_text(&source);
            let text_start = text.len();
            text.push_str(cell_text);
            cells.push(SnapshotCell {
                text_start: u32::try_from(text_start).unwrap_or(u32::MAX),
                text_len: u16::try_from(cell_text.len()).unwrap_or(0),
                fg: source.fg,
                bg: source.bg,
                attrs: source.attrs,
                hyperlink_idx: source.hyperlink_idx,
            });
        }
        Some(Self {
            row,
            meta: snapshot
                .rows_meta
                .get(usize::from(row))
                .copied()
                .unwrap_or_default(),
            cells,
            text,
        })
    }

    fn from_snapshot_row_remapping_links(
        snapshot: &VtSnapshot,
        row: u16,
        hyperlinks: &mut Vec<String>,
    ) -> Option<Self> {
        if row >= snapshot.rows {
            return None;
        }
        let mut text = String::new();
        let mut cells = Vec::with_capacity(usize::from(snapshot.cols));
        for col in 0..snapshot.cols {
            let source = *snapshot.cell_at(row, col)?;
            let cell_text = snapshot.cell_text(&source);
            let text_start = text.len();
            text.push_str(cell_text);
            let hyperlink_idx = snapshot
                .cell_hyperlink(&source)
                .map_or(NO_HYPERLINK, |url| intern_hyperlink(hyperlinks, url));
            cells.push(SnapshotCell {
                text_start: u32::try_from(text_start).unwrap_or(u32::MAX),
                text_len: u16::try_from(cell_text.len()).unwrap_or(0),
                fg: source.fg,
                bg: source.bg,
                attrs: source.attrs,
                hyperlink_idx,
            });
        }
        Some(Self {
            row,
            meta: snapshot
                .rows_meta
                .get(usize::from(row))
                .copied()
                .unwrap_or_default(),
            cells,
            text,
        })
    }
}

fn intern_hyperlink(hyperlinks: &mut Vec<String>, url: &str) -> u16 {
    if let Some(idx) = hyperlinks.iter().position(|existing| existing == url) {
        return u16::try_from(idx).unwrap_or(NO_HYPERLINK);
    }
    let idx = hyperlinks.len();
    if idx >= usize::from(NO_HYPERLINK) {
        return NO_HYPERLINK;
    }
    hyperlinks.push(url.to_owned());
    u16::try_from(idx).unwrap_or(NO_HYPERLINK)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameApplyError {
    NeedFull,
    BaseGenerationMismatch {
        expected: u64,
        actual: u64,
    },
    DimensionMismatch,
    InvalidDirtyRows,
    InvalidRowIndex {
        row: u16,
        rows: u16,
    },
    InvalidRowCellCount {
        row: u16,
        expected: usize,
        actual: usize,
    },
    InvalidTextOffset,
    InvalidSnapshot(FrameValidationError),
}

impl fmt::Display for FrameApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedFull => f.write_str("full frame required"),
            Self::BaseGenerationMismatch { expected, actual } => {
                write!(
                    f,
                    "base generation mismatch: expected {expected}, got {actual}"
                )
            }
            Self::DimensionMismatch => f.write_str("frame dimensions do not match base"),
            Self::InvalidDirtyRows => f.write_str("dirty rows must be sorted and unique"),
            Self::InvalidRowIndex { row, rows } => {
                write!(f, "dirty row {row} outside row count {rows}")
            }
            Self::InvalidRowCellCount {
                row,
                expected,
                actual,
            } => write!(f, "dirty row {row} has {actual} cells, expected {expected}"),
            Self::InvalidTextOffset => f.write_str("row text offset is invalid"),
            Self::InvalidSnapshot(err) => write!(f, "invalid snapshot: {err}"),
        }
    }
}

impl std::error::Error for FrameApplyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameValidationError {
    InvalidCellCount { expected: usize, actual: usize },
    InvalidRowMetaCount { expected: usize, actual: usize },
    InvalidTextOffset,
}

impl fmt::Display for FrameValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidCellCount { expected, actual } => {
                write!(f, "snapshot has {actual} cells, expected {expected}")
            }
            Self::InvalidRowMetaCount { expected, actual } => {
                write!(
                    f,
                    "snapshot has {actual} row metadata entries, expected {expected}"
                )
            }
            Self::InvalidTextOffset => f.write_str("snapshot cell text offset is invalid"),
        }
    }
}

impl std::error::Error for FrameValidationError {}

pub fn apply_frame_delta(
    previous: Option<&VtSnapshot>,
    frame: &FrameDelta,
) -> Result<VtSnapshot, FrameApplyError> {
    match frame {
        FrameDelta::Full {
            generation,
            snapshot,
        } => {
            let mut snapshot = snapshot.clone();
            snapshot.generation = *generation;
            snapshot.dirty = DirtySnapshot::Full;
            snapshot
                .validate_dimensions()
                .map_err(FrameApplyError::InvalidSnapshot)?;
            Ok(snapshot)
        }
        FrameDelta::Partial {
            base_generation,
            generation,
            cols,
            rows,
            cursor,
            modes,
            pwd,
            placements,
            dirty_rows,
            hyperlinks,
        } => {
            let base = previous.ok_or(FrameApplyError::NeedFull)?;
            if base.generation != *base_generation {
                return Err(FrameApplyError::BaseGenerationMismatch {
                    expected: *base_generation,
                    actual: base.generation,
                });
            }
            if base.cols != *cols || base.rows != *rows {
                return Err(FrameApplyError::DimensionMismatch);
            }
            validate_sorted_unique(dirty_rows)?;
            for row in dirty_rows {
                validate_row_delta(row, *cols, *rows)?;
            }

            let dirty_set: BTreeSet<u16> = dirty_rows.iter().map(|row| row.row).collect();
            let mut next = VtSnapshot {
                generation: *generation,
                cols: *cols,
                rows: *rows,
                cells: Vec::with_capacity(usize::from(*cols) * usize::from(*rows)),
                text: String::new(),
                rows_meta: Vec::with_capacity(usize::from(*rows)),
                pwd: pwd.clone(),
                cursor: *cursor,
                modes: *modes,
                dirty: if dirty_rows.is_empty() {
                    DirtySnapshot::Clean
                } else {
                    DirtySnapshot::Partial(dirty_rows.iter().map(|row| row.row).collect())
                },
                placements: placements.clone(),
                images: base.images.clone(),
                hyperlinks: hyperlinks.clone(),
            };

            for row in 0..*rows {
                let replacement = if dirty_set.contains(&row) {
                    dirty_rows.iter().find(|delta| delta.row == row)
                } else {
                    None
                };
                let meta = replacement.map_or_else(
                    || {
                        base.rows_meta
                            .get(usize::from(row))
                            .copied()
                            .unwrap_or_default()
                    },
                    |delta| delta.meta,
                );
                next.rows_meta.push(meta);
                for col in 0..*cols {
                    let (source, text) = if let Some(delta) = replacement {
                        let idx = usize::from(col);
                        let cell = delta.cells[idx];
                        let text = row_text(&delta.text, &cell)?;
                        (cell, text)
                    } else {
                        let Some(cell) = base.cell_at(row, col) else {
                            return Err(FrameApplyError::DimensionMismatch);
                        };
                        (*cell, base.cell_text(cell))
                    };
                    let text_start = next.text.len();
                    next.text.push_str(text);
                    next.cells.push(SnapshotCell {
                        text_start: u32::try_from(text_start).unwrap_or(u32::MAX),
                        text_len: source.text_len,
                        fg: source.fg,
                        bg: source.bg,
                        attrs: source.attrs,
                        hyperlink_idx: source.hyperlink_idx,
                    });
                }
            }

            Ok(next)
        }
    }
}

/// Returns true for characters that should be treated as part of a
/// "word" by double-click selection. Includes Unicode alphanumerics
/// plus a small set of identifier/path/URL punctuation. Approximates
/// tmux's default word-separators behaviour while staying useful for
/// paths and URLs. A future PR may expose this via config.
fn is_word_char(c: char) -> bool {
    if c.is_alphanumeric() {
        return true;
    }
    matches!(
        c,
        '-' | '_' | '.' | '/' | '~' | '+' | '@' | ':' | '?' | '#' | '%' | '&' | '='
    )
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

fn validate_sorted_unique(dirty_rows: &[RowDelta]) -> Result<(), FrameApplyError> {
    if dirty_rows.windows(2).any(|pair| pair[0].row >= pair[1].row) {
        return Err(FrameApplyError::InvalidDirtyRows);
    }
    Ok(())
}

fn validate_row_delta(delta: &RowDelta, cols: u16, rows: u16) -> Result<(), FrameApplyError> {
    if delta.row >= rows {
        return Err(FrameApplyError::InvalidRowIndex {
            row: delta.row,
            rows,
        });
    }
    let expected = usize::from(cols);
    if delta.cells.len() != expected {
        return Err(FrameApplyError::InvalidRowCellCount {
            row: delta.row,
            expected,
            actual: delta.cells.len(),
        });
    }
    for cell in &delta.cells {
        validate_text_range(&delta.text, cell.text_start, cell.text_len)
            .map_err(|_| FrameApplyError::InvalidTextOffset)?;
    }
    Ok(())
}

fn validate_text_range(
    text: &str,
    text_start: u32,
    text_len: u16,
) -> Result<(), FrameValidationError> {
    let start = text_start as usize;
    let end = start.saturating_add(usize::from(text_len));
    if start > text.len()
        || end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(end)
    {
        return Err(FrameValidationError::InvalidTextOffset);
    }
    Ok(())
}

fn row_text<'a>(text: &'a str, cell: &SnapshotCell) -> Result<&'a str, FrameApplyError> {
    let start = cell.text_start as usize;
    let end = start.saturating_add(usize::from(cell.text_len));
    text.get(start..end)
        .ok_or(FrameApplyError::InvalidTextOffset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    use crate::identity::ImageId;

    fn snapshot(cols: u16, rows: u16, generation: u64, texts: &[&str]) -> VtSnapshot {
        let mut snapshot = VtSnapshot::empty(cols, rows);
        snapshot.generation = generation;
        for text in texts {
            snapshot.push_cell(
                text,
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
        }
        snapshot
    }

    #[test]
    fn selection_text_matches_terminal_selection_rules() {
        let normal = snapshot(5, 1, 1, &["h", "e", "l", "l", "o"]);
        let mut sel = Selection::new(GridPos { col: 1, row: 0 });
        sel.update(GridPos { col: 3, row: 0 });
        assert_eq!(normal.selection_text(&sel).as_deref(), Some("ell"));

        let mut reversed = Selection::new(GridPos { col: 3, row: 0 });
        reversed.update(GridPos { col: 1, row: 0 });
        assert_eq!(normal.selection_text(&reversed).as_deref(), Some("ell"));

        let multiline = snapshot(
            4,
            3,
            1,
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"],
        );
        let mut sel = Selection::new(GridPos { col: 1, row: 0 });
        sel.update(GridPos { col: 2, row: 1 });
        assert_eq!(multiline.selection_text(&sel).as_deref(), Some("bcd\nefg"));

        let line = snapshot(3, 2, 1, &["a", "b", " ", "c", "d", " "]);
        let mut sel = Selection::new_line(GridPos { col: 2, row: 0 });
        sel.update(GridPos { col: 0, row: 1 });
        assert_eq!(line.selection_text(&sel).as_deref(), Some("ab\ncd"));

        let empty_cell = snapshot(3, 1, 1, &["a", "", "b"]);
        let mut sel = Selection::new(GridPos { col: 0, row: 0 });
        sel.update(GridPos { col: 2, row: 0 });
        assert_eq!(empty_cell.selection_text(&sel).as_deref(), Some("a b"));

        let empty_result = snapshot(2, 1, 1, &["", ""]);
        let mut sel = Selection::new(GridPos { col: 0, row: 0 });
        sel.update(GridPos { col: 1, row: 0 });
        assert_eq!(empty_result.selection_text(&sel), None);
    }

    #[test]
    fn frame_delta_full_apply_marks_full_dirty() {
        let snap = snapshot(1, 1, 10, &["x"]);
        let applied = apply_frame_delta(
            None,
            &FrameDelta::Full {
                generation: 10,
                snapshot: snap,
            },
        )
        .unwrap();
        assert_eq!(applied.generation, 10);
        assert_eq!(applied.dirty, DirtySnapshot::Full);
        assert_eq!(applied.cell_text(&applied.cells[0]), "x");
    }

    #[test]
    fn partial_apply_preserves_unchanged_rows_and_rewrites_offsets() {
        let base = snapshot(2, 2, 1, &["a", "b", "c", "d"]);
        let mut next = base.clone();
        next.generation = 2;
        next.text.clear();
        next.cells.clear();
        for text in ["a", "b", "é", "zz"] {
            next.push_cell(
                text,
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
        }
        next.rows_meta[1] = RowMeta {
            wrap: true,
            wrap_continuation: false,
        };
        next.dirty = DirtySnapshot::Partial(vec![1]);

        let delta = FrameDelta::from_snapshot(Some(&base), &next);
        let applied = apply_frame_delta(Some(&base), &delta).unwrap();

        assert_eq!(applied.cell_text(&applied.cells[0]), "a");
        assert_eq!(applied.cell_text(&applied.cells[1]), "b");
        assert_eq!(applied.cell_text(&applied.cells[2]), "é");
        assert_eq!(applied.cell_text(&applied.cells[3]), "zz");
        assert_eq!(applied.text, "abézz");
        assert_eq!(applied.cells[2].text_start, 2);
        assert_eq!(applied.cells[3].text_start, 4);
        assert!(applied.rows_meta[1].wrap);
        assert_eq!(applied.dirty, DirtySnapshot::Partial(vec![1]));
    }

    #[test]
    fn image_payload_change_forces_full_frame() {
        let base = snapshot(1, 1, 1, &["a"]);
        let mut next = snapshot(1, 1, 2, &["a"]);
        next.dirty = DirtySnapshot::Clean;
        next.images.push(SnapshotImage {
            image_id: ImageId(1),
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        });

        assert!(matches!(
            FrameDelta::from_snapshot(Some(&base), &next),
            FrameDelta::Full { generation: 2, .. }
        ));
    }

    #[test]
    fn row_metadata_length_is_validated() {
        let mut snap = snapshot(1, 2, 1, &["a", "b"]);
        snap.rows_meta.pop();

        assert_eq!(
            snap.validate_dimensions().unwrap_err(),
            FrameValidationError::InvalidRowMetaCount {
                expected: 2,
                actual: 1
            }
        );
    }

    #[test]
    fn partial_without_matching_base_needs_full() {
        let base = snapshot(1, 1, 1, &["a"]);
        let mut next = snapshot(1, 1, 2, &["b"]);
        next.dirty = DirtySnapshot::Partial(vec![0]);
        let delta = FrameDelta::from_snapshot(Some(&base), &next);

        assert_eq!(
            apply_frame_delta(None, &delta).unwrap_err(),
            FrameApplyError::NeedFull
        );
        let wrong = snapshot(1, 1, 0, &["a"]);
        assert_eq!(
            apply_frame_delta(Some(&wrong), &delta).unwrap_err(),
            FrameApplyError::BaseGenerationMismatch {
                expected: 1,
                actual: 0
            }
        );
    }

    #[test]
    fn partial_validation_rejects_bad_rows_and_offsets() {
        let base = snapshot(2, 1, 1, &["a", "b"]);
        let bad_rows = FrameDelta::Partial {
            base_generation: 1,
            generation: 2,
            cols: 2,
            rows: 1,
            cursor: CursorInfo::default(),
            modes: TerminalModes::default(),
            pwd: None,
            placements: Vec::new(),
            dirty_rows: vec![
                RowDelta {
                    row: 0,
                    meta: RowMeta::default(),
                    cells: vec![SnapshotCell::empty(); 2],
                    text: String::new(),
                },
                RowDelta {
                    row: 0,
                    meta: RowMeta::default(),
                    cells: vec![SnapshotCell::empty(); 2],
                    text: String::new(),
                },
            ],
            hyperlinks: Vec::new(),
        };
        assert_eq!(
            apply_frame_delta(Some(&base), &bad_rows).unwrap_err(),
            FrameApplyError::InvalidDirtyRows
        );

        let bad_offset = FrameDelta::Partial {
            base_generation: 1,
            generation: 2,
            cols: 2,
            rows: 1,
            cursor: CursorInfo::default(),
            modes: TerminalModes::default(),
            pwd: None,
            placements: Vec::new(),
            dirty_rows: vec![RowDelta {
                row: 0,
                meta: RowMeta::default(),
                cells: vec![
                    SnapshotCell {
                        text_start: 1,
                        text_len: 1,
                        ..SnapshotCell::empty()
                    },
                    SnapshotCell::empty(),
                ],
                text: "é".into(),
            }],
            hyperlinks: Vec::new(),
        };
        assert_eq!(
            apply_frame_delta(Some(&base), &bad_offset).unwrap_err(),
            FrameApplyError::InvalidTextOffset
        );
    }

    #[test]
    fn metadata_only_partial_updates_metadata_with_clean_dirty() {
        let base = snapshot(1, 1, 1, &["x"]);
        let delta = FrameDelta::Partial {
            base_generation: 1,
            generation: 2,
            cols: 1,
            rows: 1,
            cursor: CursorInfo {
                pos: GridPos { col: 0, row: 0 },
                visible: false,
                wide: false,
                shape: Some(CursorShape::Bar),
            },
            modes: TerminalModes {
                bracketed_paste: true,
                ..TerminalModes::default()
            },
            pwd: Some("/tmp".to_string()),
            placements: vec![PlacementSnapshot {
                image_id: ImageId(1),
                placement_id: 1,
                viewport_col: 0,
                viewport_row: 0,
                pixel_width: 1,
                pixel_height: 1,
                source_x: 0,
                source_y: 0,
                source_width: 1,
                source_height: 1,
                image_width: 1,
                image_height: 1,
                z: 0,
            }],
            dirty_rows: Vec::new(),
            hyperlinks: Vec::new(),
        };
        let applied = apply_frame_delta(Some(&base), &delta).unwrap();
        assert_eq!(applied.cell_text(&applied.cells[0]), "x");
        assert_eq!(applied.cursor.shape, Some(CursorShape::Bar));
        assert!(applied.modes.bracketed_paste);
        assert_eq!(applied.pwd.as_deref(), Some("/tmp"));
        assert_eq!(applied.placements.len(), 1);
        assert_eq!(applied.dirty, DirtySnapshot::Clean);
    }

    #[test]
    fn hyperlink_intern_dedupes_and_resolves_via_cell_lookup() {
        let mut snap = VtSnapshot::empty(2, 1);
        snap.push_cell(
            "a",
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        );
        let idx = snap.intern_hyperlink("https://example.com");
        snap.set_last_cell_hyperlink(idx);
        snap.push_cell(
            "b",
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        );
        let idx2 = snap.intern_hyperlink("https://example.com");
        assert_eq!(idx, idx2);
        snap.set_last_cell_hyperlink(idx2);

        let cell0 = snap.cell_at(0, 0).unwrap();
        let cell1 = snap.cell_at(0, 1).unwrap();
        assert_eq!(snap.cell_hyperlink(cell0), Some("https://example.com"));
        assert_eq!(snap.cell_hyperlink(cell1), Some("https://example.com"));
        assert_eq!(snap.hyperlinks.len(), 1);

        // Empty cells default to NO_HYPERLINK and resolve to None.
        let mut bare = VtSnapshot::empty(1, 1);
        bare.push_empty_cell();
        let cell = bare.cell_at(0, 0).unwrap();
        assert_eq!(cell.hyperlink_idx, NO_HYPERLINK);
        assert_eq!(bare.cell_hyperlink(cell), None);
    }

    #[test]
    fn osc8_run_at_walks_contiguous_same_index_cells() {
        // Layout: row 0 = [link_a link_a link_b link_a empty]
        // row 1   = [empty empty empty empty empty]
        let mut snap = VtSnapshot::empty(5, 2);
        let url_a = "https://example.com/a";
        let url_b = "https://example.com/b";
        let idx_a = snap.intern_hyperlink(url_a);
        let idx_b = snap.intern_hyperlink(url_b);
        let mut push = |idx: u16| {
            snap.push_cell(
                "x",
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            );
            if idx != NO_HYPERLINK {
                snap.set_last_cell_hyperlink(idx);
            }
        };
        push(idx_a);
        push(idx_a);
        push(idx_b);
        push(idx_a);
        push(NO_HYPERLINK);
        for _ in 0..5 {
            snap.push_empty_cell();
        }

        // Hover on col 0: run spans 0..=1 (the b at col 2 breaks it).
        let run = snap.osc8_run_at(0, 0).unwrap();
        assert_eq!((run.start_col, run.end_col, run.url), (0, 1, url_a));

        // Hover on col 1: same run, found by walking left.
        let run = snap.osc8_run_at(1, 0).unwrap();
        assert_eq!((run.start_col, run.end_col, run.url), (0, 1, url_a));

        // Hover on col 2 (different URL): single-cell run with url_b.
        let run = snap.osc8_run_at(2, 0).unwrap();
        assert_eq!((run.start_col, run.end_col, run.url), (2, 2, url_b));

        // Hover on col 3 (url_a again): single-cell run; the run does not
        // bridge across the url_b cell.
        let run = snap.osc8_run_at(3, 0).unwrap();
        assert_eq!((run.start_col, run.end_col, run.url), (3, 3, url_a));

        // No hyperlink at col 4 or anywhere on row 1.
        assert!(snap.osc8_run_at(4, 0).is_none());
        assert!(snap.osc8_run_at(0, 1).is_none());
    }

    #[test]
    fn frame_delta_partial_round_trips_hyperlink_table() {
        let mut base = snapshot(1, 2, 1, &["a", "b"]);
        let mut next = base.clone();
        next.generation = 2;
        next.dirty = DirtySnapshot::Partial(vec![0]);
        let url_idx = next.intern_hyperlink("https://example.com/x");
        // Replace cell 0 with a hyperlinked variant.
        next.cells[0].hyperlink_idx = url_idx;
        base.dirty = DirtySnapshot::Clean;

        let delta = FrameDelta::from_snapshot(Some(&base), &next);
        assert!(matches!(delta, FrameDelta::Partial { .. }));

        let applied = apply_frame_delta(Some(&base), &delta).unwrap();
        assert_eq!(applied.hyperlinks, vec!["https://example.com/x".to_owned()]);
        let cell = applied.cell_at(0, 0).unwrap();
        assert_eq!(applied.cell_hyperlink(cell), Some("https://example.com/x"));
        let other = applied.cell_at(0, 1).unwrap();
        assert_eq!(applied.cell_hyperlink(other), None);
    }

    #[test]
    fn word_range_at_expands_across_word_chars_only() {
        // "hello world/path one"
        let snap = snapshot(
            20,
            1,
            1,
            &[
                "h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d", "/", "p", "a", "t", "h",
                " ", "o", "n", "e",
            ],
        );
        // Click inside "hello".
        let (s, e) = snap.word_range_at(0, 2).unwrap();
        assert_eq!((s.col, e.col), (0, 4));
        // Click on the trailing 'o' of hello.
        let (s, e) = snap.word_range_at(0, 4).unwrap();
        assert_eq!((s.col, e.col), (0, 4));
        // Click on space → None.
        assert!(snap.word_range_at(0, 5).is_none());
        // Click inside "world/path" — the slash is part of the word class.
        let (s, e) = snap.word_range_at(0, 9).unwrap();
        assert_eq!((s.col, e.col), (6, 15));
        // Click on the slash itself — still part of the word.
        let (s, e) = snap.word_range_at(0, 11).unwrap();
        assert_eq!((s.col, e.col), (6, 15));
        // Last word reaches the row's end.
        let (s, e) = snap.word_range_at(0, 18).unwrap();
        assert_eq!((s.col, e.col), (17, 19));
        // Out-of-bounds row → None.
        assert!(snap.word_range_at(5, 0).is_none());
    }

    #[test]
    fn wrap_chain_walks_continuation_metadata_in_both_directions() {
        let mut snap = snapshot(1, 4, 1, &["a", "b", "c", "d"]);
        // Rows 0–2 form one logical line; row 3 is its own line.
        snap.rows_meta[0] = RowMeta {
            wrap: true,
            wrap_continuation: false,
        };
        snap.rows_meta[1] = RowMeta {
            wrap: true,
            wrap_continuation: true,
        };
        snap.rows_meta[2] = RowMeta {
            wrap: false,
            wrap_continuation: true,
        };
        snap.rows_meta[3] = RowMeta::default();

        assert_eq!(snap.wrap_chain(0), (0, 2));
        assert_eq!(snap.wrap_chain(1), (0, 2));
        assert_eq!(snap.wrap_chain(2), (0, 2));
        assert_eq!(snap.wrap_chain(3), (3, 3));
    }

    #[test]
    fn line_range_selection_spans_wrap_chain() {
        let mut snap = snapshot(2, 3, 1, &["a", "b", "c", "d", "e", "f"]);
        snap.rows_meta[0] = RowMeta {
            wrap: true,
            wrap_continuation: false,
        };
        snap.rows_meta[1] = RowMeta {
            wrap: false,
            wrap_continuation: true,
        };
        snap.rows_meta[2] = RowMeta::default();
        let sel = Selection::line_range(GridPos { col: 0, row: 0 }, GridPos { col: 1, row: 1 });
        assert_eq!(snap.selection_text(&sel).as_deref(), Some("ab\ncd"));
    }

    #[test]
    fn frame_delta_partial_keeps_unchanged_row_hyperlinks_valid() {
        let mut base = snapshot(1, 2, 1, &["a", "b"]);
        let base_idx = base.intern_hyperlink("https://example.com/base");
        base.cells[1].hyperlink_idx = base_idx;

        let mut next = snapshot(1, 2, 2, &["x", "b"]);
        let new_idx = next.intern_hyperlink("https://example.com/new");
        next.cells[0].hyperlink_idx = new_idx;
        let base_idx = next.intern_hyperlink("https://example.com/base");
        next.cells[1].hyperlink_idx = base_idx;
        next.dirty = DirtySnapshot::Partial(vec![0]);

        let delta = FrameDelta::from_snapshot(Some(&base), &next);
        let applied = apply_frame_delta(Some(&base), &delta).unwrap();

        let dirty = applied.cell_at(0, 0).unwrap();
        let unchanged = applied.cell_at(1, 0).unwrap();
        assert_eq!(
            applied.cell_hyperlink(dirty),
            Some("https://example.com/new")
        );
        assert_eq!(
            applied.cell_hyperlink(unchanged),
            Some("https://example.com/base")
        );
    }
}

#[cfg(test)]
mod prop_tests {
    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;

    fn arb_cell_color() -> impl Strategy<Value = CellColor> {
        prop_oneof![
            Just(CellColor::Default),
            any::<u8>().prop_map(CellColor::Palette),
            (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(r, g, b)| CellColor::Rgb(r, g, b)),
        ]
    }

    fn arb_attrs() -> impl Strategy<Value = CellAttrs> {
        any::<[bool; 5]>().prop_map(|[bold, italic, faint, inverse, invisible]| CellAttrs {
            bold,
            italic,
            faint,
            inverse,
            invisible,
        })
    }

    fn arb_snapshot() -> impl Strategy<Value = VtSnapshot> {
        (1u16..=6, 1u16..=6, any::<u64>()).prop_flat_map(|(cols, rows, generation)| {
            let len = usize::from(cols) * usize::from(rows);
            let cell = (
                "[a-z0-9 ]{0,2}",
                arb_cell_color(),
                arb_cell_color(),
                arb_attrs(),
            );
            (vec(cell, len..=len), vec(any::<bool>(), 0..=3)).prop_map(
                move |(cells, link_flags)| {
                    let mut snap = VtSnapshot::empty(cols, rows);
                    snap.generation = generation;
                    let urls = ["https://a.example", "https://b.example"];
                    let link_indices: Vec<u16> = link_flags
                        .iter()
                        .enumerate()
                        .map(|(i, _)| snap.intern_hyperlink(urls[i % urls.len()]))
                        .collect();
                    for (i, (text, fg, bg, attrs)) in cells.into_iter().enumerate() {
                        snap.push_cell(&text, fg, bg, attrs);
                        if !link_indices.is_empty() && i % 3 == 0 {
                            let idx = link_indices[i % link_indices.len()];
                            snap.set_last_cell_hyperlink(idx);
                        }
                    }
                    snap
                },
            )
        })
    }

    proptest! {
        #[test]
        fn postcard_roundtrips_vt_snapshot(snap in arb_snapshot()) {
            let bytes = postcard::to_allocvec(&snap).expect("serialize");
            let decoded: VtSnapshot = postcard::from_bytes(&bytes).expect("deserialize");
            prop_assert_eq!(snap, decoded);
        }

        #[test]
        fn postcard_roundtrips_frame_delta_full(snap in arb_snapshot()) {
            let generation = snap.generation;
            let frame = FrameDelta::Full { generation, snapshot: snap };
            let bytes = postcard::to_allocvec(&frame).expect("serialize");
            let decoded: FrameDelta = postcard::from_bytes(&bytes).expect("deserialize");
            prop_assert_eq!(frame, decoded);
        }

        #[test]
        fn full_frame_apply_preserves_payload(snap in arb_snapshot()) {
            let frame = FrameDelta::Full {
                generation: snap.generation,
                snapshot: snap.clone(),
            };
            let applied = apply_frame_delta(None, &frame).expect("apply");
            prop_assert_eq!(applied.generation, snap.generation);
            prop_assert_eq!(applied.cells, snap.cells);
            prop_assert_eq!(applied.text, snap.text);
            prop_assert_eq!(applied.hyperlinks, snap.hyperlinks);
            prop_assert_eq!(applied.dirty, DirtySnapshot::Full);
        }
    }
}
