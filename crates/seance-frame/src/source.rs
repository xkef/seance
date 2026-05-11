use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, DirtySnapshot, GridPos, PlacementSnapshot,
};
use seance_protocol::identity::{ImageId, PaneRef};

use crate::PlacementLayer;

pub struct ImageInfo<'a> {
    pub image_id: ImageId,
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

pub struct CellView<'a> {
    pub text: &'a str,
    pub fg: CellColor,
    pub bg: CellColor,
    pub attrs: CellAttrs,
    /// Resolved OSC 8 hyperlink URL, if the cell carries one.
    pub hyperlink: Option<&'a str>,
}

pub trait FrameSource {
    fn pane_ref(&mut self) -> PaneRef {
        PaneRef::LOCAL
    }

    fn grid_size(&mut self) -> (u16, u16);

    fn cursor(&mut self) -> CursorInfo;

    fn selection(&mut self) -> Option<(GridPos, GridPos)>;

    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor);

    fn dirty_rows(&mut self) -> DirtySnapshot {
        DirtySnapshot::Full
    }

    fn clear_dirty(&mut self) {}

    fn visit_placements(&mut self, _layer: PlacementLayer, _visitor: &mut dyn PlacementVisitor) {}

    fn visit_images(&mut self, _visitor: &mut dyn ImageVisitor) {}
}

pub trait CellVisitor {
    fn cell(&mut self, row: u16, col: u16, view: CellView<'_>);
}

pub trait PlacementVisitor {
    fn placement(&mut self, p: &PlacementSnapshot);
}

pub trait ImageVisitor {
    fn image(&mut self, info: &ImageInfo<'_>);
}
