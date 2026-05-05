//! Owned VT snapshots and render-facing adapter.
//!
//! The IO actor publishes [`VtSnapshot`] values after copying the renderable
//! state out of libghostty-owned storage. UI/render code consumes snapshots via
//! [`SnapshotFrameSource`] and never borrows live VT state.

use crate::frame::{
    CellAttrs, CellColor, CellView, CellVisitor, CursorInfo, DirtySnapshot, FrameSource, ImageInfo,
    ImageVisitor, PlacementLayer, PlacementSnapshot, PlacementVisitor,
};
use crate::modes::TerminalModes;
use crate::selection::{GridPos, Selection, SelectionGranularity};

/// Immutable, séance-owned terminal state for one render/copy handoff.
///
/// Cells are row-major and store byte offsets into [`Self::text`] instead of
/// allocating a string per cell. `generation` is assigned by VT Core and used
/// to acknowledge dirty rows after a successful render/present.
#[derive(Debug, Clone)]
pub struct VtSnapshot {
    pub generation: u64,
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<SnapshotCell>,
    pub text: String,
    pub cursor: CursorInfo,
    pub modes: TerminalModes,
    pub dirty: DirtySnapshot,
    pub placements: Vec<PlacementSnapshot>,
    pub images: Vec<SnapshotImage>,
}

impl VtSnapshot {
    /// Build an empty snapshot with capacity for `cols * rows` cells.
    pub fn empty(cols: u16, rows: u16) -> Self {
        Self {
            generation: 0,
            cols,
            rows,
            cells: Vec::with_capacity(usize::from(cols) * usize::from(rows)),
            text: String::new(),
            cursor: CursorInfo::default(),
            modes: TerminalModes::default(),
            dirty: DirtySnapshot::Full,
            placements: Vec::new(),
            images: Vec::new(),
        }
    }

    /// Return the text slice for a cell, or an empty string if the snapshot is
    /// malformed. Valid snapshots only contain UTF-8 boundary offsets.
    pub fn cell_text(&self, cell: &SnapshotCell) -> &str {
        let start = cell.text_start as usize;
        let end = start.saturating_add(usize::from(cell.text_len));
        self.text.get(start..end).unwrap_or("")
    }

    /// Extract selected text from this snapshot.
    ///
    /// Mirrors the existing live-terminal selection behavior: ordered range,
    /// line granularity selecting full rows, empty cells contributing spaces,
    /// per-row trailing whitespace trimming, and `None` for an empty result.
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

    pub(crate) fn push_cell(&mut self, text: &str, fg: CellColor, bg: CellColor, attrs: CellAttrs) {
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
        });
    }

    pub(crate) fn push_empty_cell(&mut self) {
        self.cells.push(SnapshotCell::empty());
    }

    fn cell_at(&self, row: u16, col: u16) -> Option<&SnapshotCell> {
        let idx = usize::from(row) * usize::from(self.cols) + usize::from(col);
        self.cells.get(idx)
    }
}

/// One row-major cell in an owned snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotCell {
    pub text_start: u32,
    pub text_len: u16,
    pub fg: CellColor,
    pub bg: CellColor,
    pub attrs: CellAttrs,
}

impl SnapshotCell {
    pub fn empty() -> Self {
        Self {
            text_start: 0,
            text_len: 0,
            fg: CellColor::Default,
            bg: CellColor::Default,
            attrs: CellAttrs::default(),
        }
    }
}

/// One copied Kitty graphics image payload in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotImage {
    pub image_id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Borrowed [`FrameSource`] view over an immutable [`VtSnapshot`].
pub struct SnapshotFrameSource<'a> {
    snapshot: &'a VtSnapshot,
}

impl<'a> SnapshotFrameSource<'a> {
    pub fn new(snapshot: &'a VtSnapshot) -> Self {
        Self { snapshot }
    }
}

impl FrameSource for SnapshotFrameSource<'_> {
    fn grid_size(&mut self) -> (u16, u16) {
        (self.snapshot.cols, self.snapshot.rows)
    }

    fn cursor(&mut self) -> CursorInfo {
        self.snapshot.cursor
    }

    fn selection(&mut self) -> Option<(GridPos, GridPos)> {
        None
    }

    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor) {
        for row in 0..self.snapshot.rows {
            for col in 0..self.snapshot.cols {
                let Some(cell) = self.snapshot.cell_at(row, col) else {
                    continue;
                };
                visitor.cell(
                    row,
                    col,
                    CellView {
                        text: self.snapshot.cell_text(cell),
                        fg: cell.fg,
                        bg: cell.bg,
                        attrs: cell.attrs,
                    },
                );
            }
        }
    }

    fn dirty_rows(&mut self) -> DirtySnapshot {
        self.snapshot.dirty.clone()
    }

    fn clear_dirty(&mut self) {}

    fn visit_placements(&mut self, layer: PlacementLayer, visitor: &mut dyn PlacementVisitor) {
        for placement in &self.snapshot.placements {
            if layer.contains_z(placement.z) {
                visitor.placement(placement);
            }
        }
    }

    fn visit_images(&mut self, visitor: &mut dyn ImageVisitor) {
        for image in &self.snapshot.images {
            visitor.image(&ImageInfo {
                image_id: image.image_id,
                width: image.width,
                height: image.height,
                rgba: &image.rgba,
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{CursorShape, ImageVisitor, PlacementVisitor};

    fn snapshot_with_cells(cols: u16, rows: u16, texts: &[&str]) -> VtSnapshot {
        let mut snapshot = VtSnapshot::empty(cols, rows);
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

    #[derive(Default)]
    struct Cells(Vec<(u16, u16, String, CellColor, CellColor, CellAttrs)>);

    impl CellVisitor for Cells {
        fn cell(&mut self, row: u16, col: u16, view: CellView<'_>) {
            self.0
                .push((row, col, view.text.to_owned(), view.fg, view.bg, view.attrs));
        }
    }

    #[test]
    fn snapshot_frame_source_exposes_snapshot_state() {
        let mut snapshot = VtSnapshot::empty(2, 2);
        let attrs = CellAttrs {
            bold: true,
            italic: false,
            faint: false,
            inverse: true,
            invisible: false,
        };
        snapshot.push_cell("A", CellColor::Rgb(1, 2, 3), CellColor::Default, attrs);
        snapshot.push_cell(
            "",
            CellColor::Default,
            CellColor::Palette(4),
            CellAttrs::default(),
        );
        snapshot.push_cell(
            "β",
            CellColor::Palette(9),
            CellColor::Default,
            CellAttrs::default(),
        );
        snapshot.push_cell(
            "CD",
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        );
        snapshot.cursor = CursorInfo {
            pos: GridPos { col: 1, row: 0 },
            visible: false,
            wide: true,
            shape: Some(CursorShape::Bar),
        };
        snapshot.modes = TerminalModes {
            cursor_keys: true,
            mouse_tracking: true,
            mouse_format_sgr: false,
            bracketed_paste: true,
        };
        snapshot.dirty = DirtySnapshot::Partial(vec![1]);
        snapshot.placements.push(PlacementSnapshot {
            image_id: 7,
            placement_id: 11,
            viewport_col: 1,
            viewport_row: 2,
            pixel_width: 30,
            pixel_height: 40,
            source_x: 0,
            source_y: 1,
            source_width: 3,
            source_height: 4,
            image_width: 10,
            image_height: 20,
            z: -1,
        });
        snapshot.images.push(SnapshotImage {
            image_id: 7,
            width: 10,
            height: 20,
            rgba: vec![1, 2, 3, 4],
        });

        let mut source = SnapshotFrameSource::new(&snapshot);
        assert_eq!(source.grid_size(), (2, 2));
        assert_eq!(source.cursor(), snapshot.cursor);
        assert_eq!(source.selection(), None);
        assert_eq!(source.dirty_rows(), DirtySnapshot::Partial(vec![1]));

        let mut cells = Cells::default();
        source.visit_cells(&mut cells);
        assert_eq!(
            cells.0,
            vec![
                (
                    0,
                    0,
                    "A".into(),
                    CellColor::Rgb(1, 2, 3),
                    CellColor::Default,
                    attrs
                ),
                (
                    0,
                    1,
                    "".into(),
                    CellColor::Default,
                    CellColor::Palette(4),
                    CellAttrs::default(),
                ),
                (
                    1,
                    0,
                    "β".into(),
                    CellColor::Palette(9),
                    CellColor::Default,
                    CellAttrs::default(),
                ),
                (
                    1,
                    1,
                    "CD".into(),
                    CellColor::Default,
                    CellColor::Default,
                    CellAttrs::default(),
                ),
            ]
        );
    }

    #[test]
    fn snapshot_frame_source_clear_dirty_is_noop() {
        let mut snapshot = snapshot_with_cells(1, 1, &["x"]);
        snapshot.dirty = DirtySnapshot::Partial(vec![0]);

        let mut source = SnapshotFrameSource::new(&snapshot);
        assert_eq!(source.dirty_rows(), DirtySnapshot::Partial(vec![0]));
        source.clear_dirty();
        assert_eq!(source.dirty_rows(), DirtySnapshot::Partial(vec![0]));
        assert_eq!(snapshot.dirty, DirtySnapshot::Partial(vec![0]));
    }

    #[test]
    fn selection_text_matches_live_terminal_selection_rules() {
        let normal = snapshot_with_cells(5, 1, &["h", "e", "l", "l", "o"]);
        let mut sel = Selection::new(GridPos { col: 1, row: 0 });
        sel.update(GridPos { col: 3, row: 0 });
        assert_eq!(normal.selection_text(&sel).as_deref(), Some("ell"));

        let mut reversed = Selection::new(GridPos { col: 3, row: 0 });
        reversed.update(GridPos { col: 1, row: 0 });
        assert_eq!(normal.selection_text(&reversed).as_deref(), Some("ell"));

        let multiline = snapshot_with_cells(
            4,
            3,
            &["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"],
        );
        let mut sel = Selection::new(GridPos { col: 1, row: 0 });
        sel.update(GridPos { col: 2, row: 1 });
        assert_eq!(multiline.selection_text(&sel).as_deref(), Some("bcd\nefg"));

        let line = snapshot_with_cells(3, 2, &["a", "b", " ", "c", "d", " "]);
        let mut sel = Selection::new_line(GridPos { col: 2, row: 0 });
        sel.update(GridPos { col: 0, row: 1 });
        assert_eq!(line.selection_text(&sel).as_deref(), Some("ab\ncd"));

        let empty_cell = snapshot_with_cells(3, 1, &["a", "", "b"]);
        let mut sel = Selection::new(GridPos { col: 0, row: 0 });
        sel.update(GridPos { col: 2, row: 0 });
        assert_eq!(empty_cell.selection_text(&sel).as_deref(), Some("a b"));

        let empty_result = snapshot_with_cells(2, 1, &["", ""]);
        let mut sel = Selection::new(GridPos { col: 0, row: 0 });
        sel.update(GridPos { col: 1, row: 0 });
        assert_eq!(empty_result.selection_text(&sel), None);
    }

    #[derive(Default)]
    struct Placements(Vec<PlacementSnapshot>);

    impl PlacementVisitor for Placements {
        fn placement(&mut self, p: &PlacementSnapshot) {
            self.0.push(*p);
        }
    }

    struct Images<'a> {
        expected_ptr: *const u8,
        seen_borrowed_payload: bool,
        seen: Vec<(u32, u32, u32, Vec<u8>)>,
        _marker: std::marker::PhantomData<&'a ()>,
    }

    impl ImageVisitor for Images<'_> {
        fn image(&mut self, info: &ImageInfo<'_>) {
            self.seen_borrowed_payload = info.rgba.as_ptr() == self.expected_ptr;
            self.seen
                .push((info.image_id, info.width, info.height, info.rgba.to_vec()));
        }
    }

    #[test]
    fn image_and_placement_visitors_borrow_and_filter_snapshot_data() {
        let mut snapshot = snapshot_with_cells(1, 1, &["x"]);
        let below_bg = PlacementSnapshot {
            image_id: 1,
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
            z: i32::MIN / 2 - 1,
        };
        let below_text = PlacementSnapshot { z: -1, ..below_bg };
        let above_text = PlacementSnapshot { z: 0, ..below_bg };
        snapshot.placements = vec![below_bg, below_text, above_text];
        snapshot.images = vec![SnapshotImage {
            image_id: 42,
            width: 2,
            height: 1,
            rgba: vec![9, 8, 7, 6, 5, 4, 3, 2],
        }];

        let mut source = SnapshotFrameSource::new(&snapshot);

        let mut placements = Placements::default();
        source.visit_placements(PlacementLayer::BelowBg, &mut placements);
        assert_eq!(placements.0, vec![below_bg]);

        let mut placements = Placements::default();
        source.visit_placements(PlacementLayer::BelowText, &mut placements);
        assert_eq!(placements.0, vec![below_text]);

        let mut placements = Placements::default();
        source.visit_placements(PlacementLayer::AboveText, &mut placements);
        assert_eq!(placements.0, vec![above_text]);

        let expected_ptr = snapshot.images[0].rgba.as_ptr();
        let mut images = Images {
            expected_ptr,
            seen_borrowed_payload: false,
            seen: Vec::new(),
            _marker: std::marker::PhantomData,
        };
        source.visit_images(&mut images);
        assert!(images.seen_borrowed_payload);
        assert_eq!(images.seen, vec![(42, 2, 1, vec![9, 8, 7, 6, 5, 4, 3, 2])]);
    }
}
