use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, DirtySnapshot, GridPos, PlacementSnapshot,
};
use seance_protocol::identity::{ImageId, PaneRef};

use crate::PlacementLayer;

/// Borrowed view of an image payload available for the current frame.
/// The `rgba` slice's lifetime is the frame — visitors may not retain it.
pub struct ImageInfo<'a> {
    pub image_id: ImageId,
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// Borrowed view of a single grid cell. Lifetime is the frame; visitors
/// must copy any fields they need to retain.
pub struct CellView<'a> {
    pub text: &'a str,
    pub fg: CellColor,
    pub bg: CellColor,
    pub attrs: CellAttrs,
    /// Resolved OSC 8 hyperlink URL, if the cell carries one.
    pub hyperlink: Option<&'a str>,
}

/// The seam the renderer reads each frame.
///
/// Implementors expose the current grid, cursor, selection, dirty rows,
/// and any image / placement data through visitor callbacks rather than
/// owned collections so the hot path never allocates per-cell. The
/// default `dirty_rows`/`visit_images`/`visit_placements` impls make the
/// trait easy to fulfill for headless or stub sources.
pub trait FrameSource {
    /// Which pane this frame belongs to. Defaults to [`PaneRef::LOCAL`]
    /// for tests; production sources override this to return the real
    /// pane ref so the renderer can scope its caches per pane.
    fn pane_ref(&mut self) -> PaneRef {
        PaneRef::LOCAL
    }

    /// `(cols, rows)` of the current grid.
    fn grid_size(&mut self) -> (u16, u16);

    /// Current cursor position, shape, and visibility.
    fn cursor(&mut self) -> CursorInfo;

    /// Active selection range, if any. `(start, end)` are grid positions
    /// in row-major reading order.
    fn selection(&mut self) -> Option<(GridPos, GridPos)>;

    /// Walk every cell the renderer needs to repaint this frame, handing
    /// each to `visitor`.
    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor);

    /// Which rows actually changed since the last frame. Defaults to
    /// `Full` (repaint everything); production sources override to
    /// enable partial uploads.
    fn dirty_rows(&mut self) -> DirtySnapshot {
        DirtySnapshot::Full
    }

    /// Acknowledge that the renderer has consumed the dirty set; the
    /// next call to `dirty_rows` reports only rows changed *after* this
    /// point. Defaults to no-op.
    fn clear_dirty(&mut self) {}

    /// Walk image placements for `layer`. Defaults to no-op so sources
    /// without graphics support need not implement this.
    fn visit_placements(&mut self, _layer: PlacementLayer, _visitor: &mut dyn PlacementVisitor) {}

    /// Walk image payloads needed for the current frame. Defaults to
    /// no-op.
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
