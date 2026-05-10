use std::collections::BTreeSet;
use std::fmt;
use std::sync::mpsc;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_big_array::BigArray;

pub const CURRENT_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);
pub const MIN_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion(1);
pub const MAX_DECODED_MESSAGE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_PTY_INPUT_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_INPUT_BYTES_PER_CLIENT: usize = 4 * 1024 * 1024;
pub const MAX_IMAGE_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING_OUTBOUND_BYTES_PER_CLIENT: usize = 32 * 1024 * 1024;
pub const MAX_RETAINED_PANE_UPDATES: usize = 512;

/// Sentinel for [`SnapshotCell::hyperlink_idx`] meaning "this cell has no
/// OSC 8 hyperlink." Real indices reference [`VtSnapshot::hyperlinks`].
pub const NO_HYPERLINK: u16 = u16::MAX;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ProtocolVersion(pub u16);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Capability {
    Zstd,
    FrameDelta,
    ImageCache,
    ImageChunks,
    Resume,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct RequestId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct StreamId(pub u16);

impl StreamId {
    pub const CONTROL: Self = Self(0);
    pub const INPUT: Self = Self(1);
    pub const OUTPUT: Self = Self(2);
    pub const IMAGES: Self = Self(3);
}

impl RequestId {
    pub const PUSH: Self = Self(0);

    pub fn is_push(self) -> bool {
        self.0 == 0
    }
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerSeq(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Generation(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ServerId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct SessionId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ClientId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct DomainId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct WindowId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct TabId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneId(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct PaneEpoch(pub u64);

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ImageId(pub u64);

impl From<u32> for ImageId {
    fn from(value: u32) -> Self {
        Self(u64::from(value))
    }
}

impl From<u64> for ImageId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneRef {
    pub pane_id: PaneId,
    pub epoch: PaneEpoch,
}

impl PaneRef {
    pub const LOCAL: Self = Self {
        pane_id: PaneId(0),
        epoch: PaneEpoch(0),
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImageKey {
    pub pane: PaneRef,
    pub image_id: ImageId,
}

impl ImageKey {
    pub fn local(image_id: ImageId) -> Self {
        Self {
            pane: PaneRef::LOCAL,
            image_id,
        }
    }
}

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

    pub fn new_word(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Word)
    }

    pub fn new_line(pos: GridPos) -> Self {
        Self::at(pos, SelectionGranularity::Line)
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
    pub mouse_tracking: bool,
    pub mouse_format_sgr: bool,
    pub bracketed_paste: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFormat {
    Rgba8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePayload {
    pub key: ImageKey,
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
    pub format: ImageFormat,
    pub digest: [u8; 32],
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePutStart {
    pub key: ImageKey,
    pub width: u32,
    pub height: u32,
    pub byte_len: u64,
    pub format: ImageFormat,
    pub digest: [u8; 32],
    pub chunk_bytes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImagePutChunk {
    pub key: ImageKey,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageCacheEvent {
    Put(ImagePayload),
    PutStart(ImagePutStart),
    PutChunk(ImagePutChunk),
    PutComplete { key: ImageKey },
    Evict { key: ImageKey },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub min_version: ProtocolVersion,
    pub max_version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
    pub max_message_bytes: u32,
    pub max_image_bytes: u64,
    pub last_seen_seq: Option<ServerSeq>,
}

impl Default for Hello {
    fn default() -> Self {
        Self {
            min_version: MIN_PROTOCOL_VERSION,
            max_version: CURRENT_PROTOCOL_VERSION,
            capabilities: vec![Capability::FrameDelta, Capability::ImageCache],
            max_message_bytes: u32::try_from(MAX_DECODED_MESSAGE_BYTES).unwrap_or(u32::MAX),
            max_image_bytes: 64 * 1024 * 1024,
            last_seen_seq: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerHello {
    pub version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
    pub server_id: ServerId,
    pub session_id: SessionId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolErrorKind {
    VersionMismatch,
    UnsupportedCapability,
    UnknownMessage,
    BadRoute,
    StalePane,
    NeedFull,
    FrameTooLarge,
    ImageTooLarge,
    ProtocolCorrupt,
    PaneExited,
    TransportEof,
    Detached,
    ServerPaneError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorPayload {
    pub kind: ProtocolErrorKind,
    pub message: String,
    pub request_id: RequestId,
    pub pane: Option<PaneRef>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello(Hello),
    Subscribe {
        pane: Option<PaneRef>,
    },
    SpawnPane {
        domain: DomainId,
        cols: u16,
        rows: u16,
    },
    ClosePane {
        pane: PaneRef,
    },
    ResizePane {
        pane: PaneRef,
        resize: Resize,
    },
    PaneInput {
        pane: PaneRef,
        bytes: Vec<u8>,
    },
    RequestSnapshot {
        pane: PaneRef,
    },
    ImageCacheMiss {
        key: ImageKey,
    },
    AckApplied {
        pane: PaneRef,
        seq: ServerSeq,
    },
    AckPresented {
        pane: PaneRef,
        generation: u64,
    },
    Ping {
        nonce: u64,
    },
    GetLines {
        pane: PaneRef,
        range: LineRange,
        since_seq: Option<ServerSeq>,
    },
    ScrollPane {
        pane: PaneRef,
        delta: i32,
    },
    SetPaneTheme {
        pane: PaneRef,
        colors: ThemeColors,
    },
    SetPaneCursorShape {
        pane: PaneRef,
        shape: CursorShape,
    },
}

impl ClientMessage {
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::ClientHello,
            Self::Subscribe { .. } => MessageKind::ClientSubscribe,
            Self::SpawnPane { .. } => MessageKind::ClientSpawnPane,
            Self::ClosePane { .. } => MessageKind::ClientClosePane,
            Self::ResizePane { .. } => MessageKind::ClientResizePane,
            Self::ScrollPane { .. } => MessageKind::ClientScrollPane,
            Self::SetPaneTheme { .. } => MessageKind::ClientSetPaneTheme,
            Self::SetPaneCursorShape { .. } => MessageKind::ClientSetPaneCursorShape,
            Self::PaneInput { .. } => MessageKind::ClientPaneInput,
            Self::RequestSnapshot { .. } => MessageKind::ClientRequestSnapshot,
            Self::ImageCacheMiss { .. } => MessageKind::ClientImageCacheMiss,
            Self::AckApplied { .. } => MessageKind::ClientAckApplied,
            Self::AckPresented { .. } => MessageKind::ClientAckPresented,
            Self::Ping { .. } => MessageKind::ClientPing,
            Self::GetLines { .. } => MessageKind::ClientGetLines,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMessage {
    Hello(ServerHello),
    Error(ProtocolErrorPayload),
    Topology(Topology),
    PaneUpdate(PaneUpdate),
    PaneExited {
        pane: PaneRef,
        exit_status: Option<i32>,
    },
    ResyncRequired {
        pane: PaneRef,
        reason: String,
    },
    Pong {
        nonce: u64,
    },
    Lines(LineContent),
}

impl ServerMessage {
    pub fn kind(&self) -> MessageKind {
        match self {
            Self::Hello(_) => MessageKind::ServerHello,
            Self::Error(_) => MessageKind::ServerError,
            Self::Topology(_) => MessageKind::ServerTopology,
            Self::PaneUpdate(_) => MessageKind::ServerPaneUpdate,
            Self::PaneExited { .. } => MessageKind::ServerPaneExited,
            Self::ResyncRequired { .. } => MessageKind::ServerResyncRequired,
            Self::Pong { .. } => MessageKind::ServerPong,
            Self::Lines(_) => MessageKind::ServerLines,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineContent {
    pub pane: PaneRef,
    pub seq: ServerSeq,
    pub generation: u64,
    pub cols: u16,
    pub range: LineRange,
    pub rows: Vec<RowDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topology {
    pub domains: Vec<DomainInfo>,
    pub windows: Vec<WindowInfo>,
    pub tabs: Vec<TabInfo>,
    pub panes: Vec<PaneInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DomainInfo {
    pub domain_id: DomainId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowInfo {
    pub window_id: WindowId,
    pub domain_id: DomainId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab_id: TabId,
    pub window_id: WindowId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub pane: PaneRef,
    pub tab_id: TabId,
    pub cols: u16,
    pub rows: u16,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneUpdate {
    pub pane: PaneRef,
    pub seq: ServerSeq,
    pub image_events: Vec<ImageCacheEvent>,
    pub frame: Option<FrameDelta>,
}

#[repr(u16)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageKind {
    ClientHello = 1,
    ClientSubscribe = 2,
    ClientSpawnPane = 3,
    ClientClosePane = 4,
    ClientResizePane = 5,
    ClientPaneInput = 6,
    ClientRequestSnapshot = 7,
    ClientImageCacheMiss = 8,
    ClientAckApplied = 9,
    ClientAckPresented = 10,
    ClientPing = 11,
    ClientGetLines = 12,
    ClientScrollPane = 13,
    ClientSetPaneTheme = 14,
    ClientSetPaneCursorShape = 15,
    ServerHello = 1001,
    ServerError = 1002,
    ServerTopology = 1003,
    ServerPaneUpdate = 1004,
    ServerPaneExited = 1005,
    ServerResyncRequired = 1006,
    ServerPong = 1007,
    ServerLines = 1008,
}

impl TryFrom<u16> for MessageKind {
    type Error = CodecError;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        let kind = match value {
            1 => Self::ClientHello,
            2 => Self::ClientSubscribe,
            3 => Self::ClientSpawnPane,
            4 => Self::ClientClosePane,
            5 => Self::ClientResizePane,
            6 => Self::ClientPaneInput,
            7 => Self::ClientRequestSnapshot,
            8 => Self::ClientImageCacheMiss,
            9 => Self::ClientAckApplied,
            10 => Self::ClientAckPresented,
            11 => Self::ClientPing,
            12 => Self::ClientGetLines,
            13 => Self::ClientScrollPane,
            14 => Self::ClientSetPaneTheme,
            15 => Self::ClientSetPaneCursorShape,
            1001 => Self::ServerHello,
            1002 => Self::ServerError,
            1003 => Self::ServerTopology,
            1004 => Self::ServerPaneUpdate,
            1005 => Self::ServerPaneExited,
            1006 => Self::ServerResyncRequired,
            1007 => Self::ServerPong,
            1008 => Self::ServerLines,
            other => return Err(CodecError::UnknownMessage(other)),
        };
        Ok(kind)
    }
}

impl From<MessageKind> for u16 {
    fn from(value: MessageKind) -> Self {
        value as u16
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportFrame {
    pub stream_id: StreamId,
    pub bytes: Vec<u8>,
}

pub trait Transport {
    fn send(&self, frame: TransportFrame) -> Result<(), TransportError>;

    fn try_recv(&self) -> Result<Option<TransportFrame>, TransportError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Closed,
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => f.write_str("transport is closed"),
        }
    }
}

impl std::error::Error for TransportError {}

pub struct InProcessTransport {
    tx: mpsc::Sender<TransportFrame>,
    rx: mpsc::Receiver<TransportFrame>,
}

impl InProcessTransport {
    pub fn pair() -> (Self, Self) {
        let (client_tx, server_rx) = mpsc::channel();
        let (server_tx, client_rx) = mpsc::channel();
        (
            Self {
                tx: client_tx,
                rx: client_rx,
            },
            Self {
                tx: server_tx,
                rx: server_rx,
            },
        )
    }
}

impl Transport for InProcessTransport {
    fn send(&self, frame: TransportFrame) -> Result<(), TransportError> {
        self.tx.send(frame).map_err(|_| TransportError::Closed)
    }

    fn try_recv(&self) -> Result<Option<TransportFrame>, TransportError> {
        match self.rx.try_recv() {
            Ok(frame) => Ok(Some(frame)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(TransportError::Closed),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub request_id: RequestId,
    pub server_seq: ServerSeq,
    pub kind: u16,
    pub payload: Vec<u8>,
}

impl Envelope {
    pub fn known_kind(&self) -> Result<MessageKind, CodecError> {
        MessageKind::try_from(self.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageDirection {
    Client,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    UnknownMessage(u16),
    OversizedFrame {
        len: usize,
        max: usize,
    },
    TruncatedFrame,
    BadCompressionFlag,
    VarintOverflow,
    CorruptPayload(String),
    UnexpectedMessageKind {
        direction: MessageDirection,
        kind: MessageKind,
    },
    WrongMessageKind {
        expected: MessageKind,
        actual: u16,
    },
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMessage(kind) => write!(f, "unknown message kind {kind}"),
            Self::OversizedFrame { len, max } => write!(f, "frame is {len} bytes, max is {max}"),
            Self::TruncatedFrame => f.write_str("truncated frame"),
            Self::BadCompressionFlag => f.write_str("compression flag is set but unsupported"),
            Self::VarintOverflow => f.write_str("frame length varint overflow"),
            Self::CorruptPayload(err) => write!(f, "corrupt payload: {err}"),
            Self::UnexpectedMessageKind { direction, kind } => {
                write!(f, "expected {direction:?} message kind, got {kind:?}")
            }
            Self::WrongMessageKind { expected, actual } => {
                write!(f, "wrong message kind: expected {expected:?}, got {actual}")
            }
        }
    }
}

impl std::error::Error for CodecError {}

pub fn encode_payload<T: Serialize>(payload: &T) -> Result<Vec<u8>, CodecError> {
    postcard::to_stdvec(payload).map_err(|err| CodecError::CorruptPayload(err.to_string()))
}

pub fn decode_payload<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    postcard::from_bytes(bytes).map_err(|err| CodecError::CorruptPayload(err.to_string()))
}

pub fn encode_envelope<T: Serialize>(
    kind: MessageKind,
    request_id: RequestId,
    server_seq: ServerSeq,
    payload: &T,
) -> Result<Vec<u8>, CodecError> {
    let envelope = Envelope {
        request_id,
        server_seq,
        kind: kind.into(),
        payload: encode_payload(payload)?,
    };
    let bytes = encode_payload(&envelope)?;
    Ok(encode_length_prefixed(&bytes, false))
}

pub fn decode_envelope(input: &[u8], max_len: usize) -> Result<(Envelope, usize), CodecError> {
    let (len, compressed, prefix_len) = decode_prefix(input)?;
    if compressed {
        return Err(CodecError::BadCompressionFlag);
    }
    if len > max_len {
        return Err(CodecError::OversizedFrame { len, max: max_len });
    }
    let end = prefix_len
        .checked_add(len)
        .ok_or(CodecError::VarintOverflow)?;
    if input.len() < end {
        return Err(CodecError::TruncatedFrame);
    }
    let envelope = decode_payload(&input[prefix_len..end])?;
    Ok((envelope, end))
}

pub fn decode_typed_payload<T: DeserializeOwned>(
    envelope: &Envelope,
    expected: MessageKind,
) -> Result<T, CodecError> {
    if envelope.kind != u16::from(expected) {
        return Err(CodecError::WrongMessageKind {
            expected,
            actual: envelope.kind,
        });
    }
    decode_payload(&envelope.payload)
}

pub fn encode_client_frame(
    message: ClientMessage,
    request_id: RequestId,
) -> Result<TransportFrame, CodecError> {
    let kind = message.kind();
    let bytes = encode_envelope(kind, request_id, ServerSeq(0), &message)?;
    Ok(TransportFrame {
        stream_id: client_stream(&message),
        bytes,
    })
}

pub fn encode_server_frame(message: ServerMessage) -> Result<TransportFrame, CodecError> {
    let kind = message.kind();
    let seq = server_seq(&message);
    let bytes = encode_envelope(kind, RequestId::PUSH, seq, &message)?;
    Ok(TransportFrame {
        stream_id: server_stream(&message),
        bytes,
    })
}

pub fn decode_client_frame(frame: &TransportFrame) -> Result<ClientMessage, CodecError> {
    let (envelope, _consumed) = decode_envelope(&frame.bytes, MAX_DECODED_MESSAGE_BYTES)?;
    let kind = envelope.known_kind()?;
    ensure_client_kind(kind)?;
    let message: ClientMessage = decode_typed_payload(&envelope, kind)?;
    if message.kind() != kind {
        return Err(CodecError::WrongMessageKind {
            expected: message.kind(),
            actual: kind.into(),
        });
    }
    Ok(message)
}

pub fn decode_server_frame(frame: &TransportFrame) -> Result<ServerMessage, CodecError> {
    let (envelope, _consumed) = decode_envelope(&frame.bytes, MAX_DECODED_MESSAGE_BYTES)?;
    let kind = envelope.known_kind()?;
    ensure_server_kind(kind)?;
    let message: ServerMessage = decode_typed_payload(&envelope, kind)?;
    if message.kind() != kind {
        return Err(CodecError::WrongMessageKind {
            expected: message.kind(),
            actual: kind.into(),
        });
    }
    Ok(message)
}

pub fn client_stream(message: &ClientMessage) -> StreamId {
    match message {
        ClientMessage::PaneInput { .. } => StreamId::INPUT,
        ClientMessage::ImageCacheMiss { .. } => StreamId::IMAGES,
        _ => StreamId::CONTROL,
    }
}

pub fn server_stream(message: &ServerMessage) -> StreamId {
    match message {
        ServerMessage::PaneUpdate(update) if !update.image_events.is_empty() => StreamId::IMAGES,
        ServerMessage::PaneUpdate(_) | ServerMessage::Lines(_) => StreamId::OUTPUT,
        _ => StreamId::CONTROL,
    }
}

pub fn server_seq(message: &ServerMessage) -> ServerSeq {
    match message {
        ServerMessage::PaneUpdate(update) => update.seq,
        ServerMessage::Lines(lines) => lines.seq,
        _ => ServerSeq(0),
    }
}

fn ensure_client_kind(kind: MessageKind) -> Result<(), CodecError> {
    match kind {
        MessageKind::ClientHello
        | MessageKind::ClientSubscribe
        | MessageKind::ClientSpawnPane
        | MessageKind::ClientClosePane
        | MessageKind::ClientResizePane
        | MessageKind::ClientPaneInput
        | MessageKind::ClientRequestSnapshot
        | MessageKind::ClientImageCacheMiss
        | MessageKind::ClientAckApplied
        | MessageKind::ClientAckPresented
        | MessageKind::ClientPing
        | MessageKind::ClientGetLines
        | MessageKind::ClientScrollPane
        | MessageKind::ClientSetPaneTheme
        | MessageKind::ClientSetPaneCursorShape => Ok(()),
        _ => Err(CodecError::UnexpectedMessageKind {
            direction: MessageDirection::Client,
            kind,
        }),
    }
}

fn ensure_server_kind(kind: MessageKind) -> Result<(), CodecError> {
    match kind {
        MessageKind::ServerHello
        | MessageKind::ServerError
        | MessageKind::ServerTopology
        | MessageKind::ServerPaneUpdate
        | MessageKind::ServerPaneExited
        | MessageKind::ServerResyncRequired
        | MessageKind::ServerPong
        | MessageKind::ServerLines => Ok(()),
        _ => Err(CodecError::UnexpectedMessageKind {
            direction: MessageDirection::Server,
            kind,
        }),
    }
}

pub fn encode_length_prefixed(payload: &[u8], compressed: bool) -> Vec<u8> {
    let value = ((payload.len() as u64) << 1) | u64::from(compressed);
    let mut out = Vec::with_capacity(varint_len(value) + payload.len());
    encode_varint(value, &mut out);
    out.extend_from_slice(payload);
    out
}

fn decode_prefix(input: &[u8]) -> Result<(usize, bool, usize), CodecError> {
    let (value, used) = decode_varint(input)?;
    let compressed = (value & 1) != 0;
    let len = usize::try_from(value >> 1).map_err(|_| CodecError::VarintOverflow)?;
    Ok((len, compressed, used))
}

fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn decode_varint(input: &[u8]) -> Result<(u64, usize), CodecError> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (idx, byte) in input.iter().copied().enumerate() {
        if idx == 10 {
            return Err(CodecError::VarintOverflow);
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, idx + 1));
        }
        shift += 7;
    }
    Err(CodecError::TruncatedFrame)
}

fn varint_len(mut value: u64) -> usize {
    let mut len = 1;
    while value >= 0x80 {
        len += 1;
        value >>= 7;
    }
    len
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

    fn pane() -> PaneRef {
        PaneRef {
            pane_id: PaneId(9),
            epoch: PaneEpoch(1),
        }
    }

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

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
    {
        let bytes = encode_payload(value).unwrap();
        let decoded = decode_payload(&bytes).unwrap();
        assert_eq!(&decoded, value);
        decoded
    }

    #[test]
    fn image_keys_scope_pane_local_ids() {
        let first = ImageKey {
            pane: PaneRef {
                pane_id: PaneId(1),
                epoch: PaneEpoch(1),
            },
            image_id: ImageId(7),
        };
        let second = ImageKey {
            pane: PaneRef {
                pane_id: PaneId(2),
                epoch: PaneEpoch(1),
            },
            image_id: ImageId(7),
        };
        assert_ne!(first, second);
        assert_eq!(ImageKey::local(ImageId(7)).pane, PaneRef::LOCAL);
    }

    #[test]
    fn protocol_transport_round_trips_typed_frames() {
        let (client, server) = InProcessTransport::pair();
        let pane = pane();
        let message = ClientMessage::PaneInput {
            pane,
            bytes: b"abc".to_vec(),
        };
        client
            .send(encode_client_frame(message.clone(), RequestId(1)).unwrap())
            .unwrap();

        let frame = server.try_recv().unwrap().unwrap();
        assert_eq!(frame.stream_id, StreamId::INPUT);
        assert_eq!(decode_client_frame(&frame).unwrap(), message);
        assert!(server.try_recv().unwrap().is_none());
    }

    #[test]
    fn protocol_decode_rejects_wrong_direction() {
        let frame = encode_server_frame(ServerMessage::Pong { nonce: 9 }).unwrap();
        assert_eq!(
            decode_client_frame(&frame).unwrap_err(),
            CodecError::UnexpectedMessageKind {
                direction: MessageDirection::Client,
                kind: MessageKind::ServerPong,
            }
        );
    }

    #[test]
    fn protocol_payloads_round_trip_through_postcard() {
        let pane = pane();
        let resize = Resize {
            cols: 80,
            rows: 24,
            pixel_width: 800,
            pixel_height: 384,
        };
        round_trip(&ClientMessage::PaneInput {
            pane,
            bytes: b"abc".to_vec(),
        });
        round_trip(&ClientMessage::ResizePane { pane, resize });
        round_trip(&ClientMessage::ScrollPane { pane, delta: -3 });
        round_trip(&ClientMessage::SetPaneTheme {
            pane,
            colors: ThemeColors {
                fg: [1, 2, 3],
                bg: [4, 5, 6],
                cursor: [7, 8, 9],
                palette: [[0, 0, 0]; 256],
            },
        });
        round_trip(&ClientMessage::SetPaneCursorShape {
            pane,
            shape: CursorShape::Underline,
        });
        round_trip(&ClientMessage::GetLines {
            pane,
            range: LineRange {
                start: 0,
                count: 24,
            },
            since_seq: Some(ServerSeq(3)),
        });
        round_trip(&ServerMessage::Hello(ServerHello {
            version: ProtocolVersion(1),
            capabilities: vec![Capability::FrameDelta],
            server_id: ServerId(1),
            session_id: SessionId(2),
        }));
        round_trip(&ServerMessage::Error(ProtocolErrorPayload {
            kind: ProtocolErrorKind::NeedFull,
            message: "base missing".into(),
            request_id: RequestId(7),
            pane: Some(pane),
        }));
        round_trip(&ServerMessage::Lines(LineContent {
            pane,
            seq: ServerSeq(4),
            generation: 9,
            cols: 1,
            range: LineRange { start: 0, count: 1 },
            rows: vec![RowDelta::from_snapshot_row(&snapshot(1, 1, 9, &["x"]), 0).unwrap()],
        }));
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
    fn snapshot_and_image_protocol_types_round_trip() {
        let mut snap = snapshot(2, 1, 3, &["α", "b"]);
        snap.cursor = CursorInfo {
            pos: GridPos { col: 1, row: 0 },
            visible: true,
            wide: false,
            shape: Some(CursorShape::Underline),
        };
        snap.modes.bracketed_paste = true;
        let pwd = std::env::temp_dir().join("seance-protocol-test-pwd");
        snap.pwd = Some(pwd.to_string_lossy().into_owned());
        snap.rows_meta[0] = RowMeta {
            wrap: true,
            wrap_continuation: false,
        };
        snap.dirty = DirtySnapshot::Partial(vec![0]);
        snap.placements.push(PlacementSnapshot {
            image_id: ImageId(42),
            placement_id: 1,
            viewport_col: 0,
            viewport_row: 0,
            pixel_width: 10,
            pixel_height: 10,
            source_x: 0,
            source_y: 0,
            source_width: 10,
            source_height: 10,
            image_width: 10,
            image_height: 10,
            z: 0,
        });
        snap.images.push(SnapshotImage {
            image_id: ImageId(42),
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 4],
        });
        round_trip(&snap);

        let key = ImageKey {
            pane: pane(),
            image_id: ImageId(42),
        };
        round_trip(&ImageCacheEvent::Put(ImagePayload {
            key,
            width: 1,
            height: 1,
            byte_len: 4,
            format: ImageFormat::Rgba8,
            digest: [5; 32],
            rgba: vec![1, 2, 3, 4],
        }));
        round_trip(&ImageCacheEvent::PutChunk(ImagePutChunk {
            key,
            offset: 0,
            bytes: vec![1, 2],
        }));
        round_trip(&ImageCacheEvent::Evict { key });
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
    fn envelope_codec_round_trips_and_has_stable_golden_bytes() {
        let encoded = encode_envelope(
            MessageKind::ClientPing,
            RequestId(7),
            ServerSeq(0),
            &ClientMessage::Ping { nonce: 99 },
        )
        .unwrap();
        assert_eq!(encoded, vec![12, 7, 0, 11, 2, 10, 99]);

        let (envelope, consumed) = decode_envelope(&encoded, MAX_DECODED_MESSAGE_BYTES).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(envelope.known_kind().unwrap(), MessageKind::ClientPing);
        let msg: ClientMessage = decode_typed_payload(&envelope, MessageKind::ClientPing).unwrap();
        assert_eq!(msg, ClientMessage::Ping { nonce: 99 });
    }

    #[test]
    fn envelope_codec_fails_cleanly() {
        let unknown = Envelope {
            request_id: RequestId(1),
            server_seq: ServerSeq(0),
            kind: 65000,
            payload: Vec::new(),
        };
        assert_eq!(
            unknown.known_kind().unwrap_err(),
            CodecError::UnknownMessage(65000)
        );

        let oversized = encode_length_prefixed(&[0; 8], false);
        assert_eq!(
            decode_envelope(&oversized, 7).unwrap_err(),
            CodecError::OversizedFrame { len: 8, max: 7 }
        );

        let truncated = encode_length_prefixed(&[1, 2, 3], false);
        assert_eq!(
            decode_envelope(&truncated[..truncated.len() - 1], MAX_DECODED_MESSAGE_BYTES)
                .unwrap_err(),
            CodecError::TruncatedFrame
        );

        let compressed = encode_length_prefixed(&[], true);
        assert_eq!(
            decode_envelope(&compressed, MAX_DECODED_MESSAGE_BYTES).unwrap_err(),
            CodecError::BadCompressionFlag
        );

        let corrupt = encode_length_prefixed(&[0xff], false);
        assert!(matches!(
            decode_envelope(&corrupt, MAX_DECODED_MESSAGE_BYTES).unwrap_err(),
            CodecError::CorruptPayload(_)
        ));
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
