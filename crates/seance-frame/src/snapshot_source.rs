use seance_protocol::frame::{CursorInfo, DirtySnapshot, GridPos, VtSnapshot};
use seance_protocol::identity::PaneRef;

use crate::{
    CellView, CellVisitor, FrameSource, ImageInfo, ImageVisitor, PlacementLayer, PlacementVisitor,
};

pub struct SnapshotFrameSource<'a> {
    snapshot: &'a VtSnapshot,
    pane: PaneRef,
}

impl<'a> SnapshotFrameSource<'a> {
    pub fn new(snapshot: &'a VtSnapshot) -> Self {
        Self::for_pane(snapshot, PaneRef::LOCAL)
    }

    pub fn for_pane(snapshot: &'a VtSnapshot, pane: PaneRef) -> Self {
        Self { snapshot, pane }
    }
}

impl FrameSource for SnapshotFrameSource<'_> {
    fn pane_ref(&mut self) -> PaneRef {
        self.pane
    }

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
                        hyperlink: self.snapshot.cell_hyperlink(cell),
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

#[cfg(test)]
mod tests {
    use super::*;
    use seance_protocol::frame::{
        CellAttrs, CellColor, CursorShape, PlacementSnapshot, SnapshotImage,
    };
    use seance_protocol::identity::{DomainId, PaneEpoch, PaneId};

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
        snapshot.dirty = DirtySnapshot::Partial(vec![1]);
        snapshot.placements.push(PlacementSnapshot {
            image_id: 7u32.into(),
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
            image_id: 7u32.into(),
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
        assert_eq!(cells.0.len(), 4);
        assert_eq!(cells.0[0].2, "A");
        assert_eq!(cells.0[2].2, "β");
    }

    #[test]
    fn snapshot_frame_source_defaults_and_overrides_pane_ref() {
        let snapshot = snapshot_with_cells(1, 1, &["x"]);
        let mut source = SnapshotFrameSource::new(&snapshot);
        assert_eq!(source.pane_ref(), PaneRef::LOCAL);

        let pane = PaneRef {
            domain: DomainId(1),
            pane_id: PaneId(7),
            epoch: PaneEpoch(3),
        };
        let mut source = SnapshotFrameSource::for_pane(&snapshot, pane);
        assert_eq!(source.pane_ref(), pane);
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

    #[derive(Default)]
    struct Placements(Vec<PlacementSnapshot>);

    impl PlacementVisitor for Placements {
        fn placement(&mut self, p: &PlacementSnapshot) {
            self.0.push(*p);
        }
    }

    struct Images {
        expected_ptr: *const u8,
        seen_borrowed_payload: bool,
        seen: Vec<(u64, u32, u32, Vec<u8>)>,
    }

    impl ImageVisitor for Images {
        fn image(&mut self, info: &ImageInfo<'_>) {
            self.seen_borrowed_payload = info.rgba.as_ptr() == self.expected_ptr;
            self.seen
                .push((info.image_id.0, info.width, info.height, info.rgba.to_vec()));
        }
    }

    #[test]
    fn image_and_placement_visitors_borrow_and_filter_snapshot_data() {
        let mut snapshot = snapshot_with_cells(1, 1, &["x"]);
        let below_bg = PlacementSnapshot {
            image_id: 1u32.into(),
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
            image_id: 42u32.into(),
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
        };
        source.visit_images(&mut images);
        assert!(images.seen_borrowed_payload);
        assert_eq!(images.seen, vec![(42, 2, 1, vec![9, 8, 7, 6, 5, 4, 3, 2])]);
    }
}
