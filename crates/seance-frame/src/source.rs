use seance_protocol::frame::{
    CellAttrs, CellColor, CursorInfo, DirtySnapshot, GridPos, PlacementSnapshot,
};
use seance_protocol::identity::{ImageId, PaneRef};

use crate::PlacementLayer;

/// Borrowed view of one cached image: identity, pixel dimensions, and
/// the underlying RGBA buffer (`width * height * 4` bytes). The slice
/// lives only for the duration of the visitor callback.
pub struct ImageInfo<'a> {
    #[allow(missing_docs)]
    pub image_id: ImageId,
    #[allow(missing_docs)]
    pub width: u32,
    #[allow(missing_docs)]
    pub height: u32,
    /// Tightly-packed RGBA bytes borrowed from the source.
    pub rgba: &'a [u8],
}

/// Borrowed view of one grid cell. The text and hyperlink slices live
/// only for the duration of the visitor callback.
pub struct CellView<'a> {
    /// Glyph text for this cell (may be empty).
    pub text: &'a str,
    #[allow(missing_docs)]
    pub fg: CellColor,
    #[allow(missing_docs)]
    pub bg: CellColor,
    #[allow(missing_docs)]
    pub attrs: CellAttrs,
    /// Resolved OSC 8 hyperlink URL, if the cell carries one.
    pub hyperlink: Option<&'a str>,
}

/// Pull-side adapter the renderer uses to read everything it needs to
/// paint a frame. Implementations may be backed by a live VT actor, a
/// stored [`seance_protocol::frame::VtSnapshot`], or a remote pane.
///
/// All methods take `&mut self` so implementations can lazily compute
/// or memoise per-frame data.
pub trait FrameSource {
    /// Identity of the pane this frame describes. Defaults to
    /// [`PaneRef::LOCAL`] for sources that have no multi-pane concept.
    fn pane_ref(&mut self) -> PaneRef {
        PaneRef::LOCAL
    }

    /// `(cols, rows)` of the grid the frame describes.
    fn grid_size(&mut self) -> (u16, u16);

    /// Cursor state for this frame.
    fn cursor(&mut self) -> CursorInfo;

    /// Active selection as `(start, end)` in row-major order, if any.
    fn selection(&mut self) -> Option<(GridPos, GridPos)>;

    /// Drive `visitor` once per cell in row-major order.
    /// Implementations may skip cells the renderer is known to ignore.
    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor);

    /// Which rows changed since the last [`Self::clear_dirty`] call.
    /// Defaults to [`DirtySnapshot::Full`] — repaint everything.
    fn dirty_rows(&mut self) -> DirtySnapshot {
        DirtySnapshot::Full
    }

    /// Mark the current dirty extent as consumed; subsequent calls to
    /// [`Self::dirty_rows`] only report changes since this call. The
    /// default impl is a no-op.
    fn clear_dirty(&mut self) {}

    /// Drive `visitor` for every placement that belongs in `layer`.
    /// The default impl emits nothing — sources without image support
    /// may ignore it.
    fn visit_placements(&mut self, _layer: PlacementLayer, _visitor: &mut dyn PlacementVisitor) {}

    /// Drive `visitor` for every image cached on the source. The
    /// default impl emits nothing.
    fn visit_images(&mut self, _visitor: &mut dyn ImageVisitor) {}
}

/// Callback that receives one cell at a time during
/// [`FrameSource::visit_cells`].
pub trait CellVisitor {
    /// Visit the cell at `(row, col)`. The borrowed view is valid only
    /// for the call duration.
    fn cell(&mut self, row: u16, col: u16, view: CellView<'_>);
}

/// Callback that receives one image placement at a time during
/// [`FrameSource::visit_placements`].
pub trait PlacementVisitor {
    /// Visit one placement.
    fn placement(&mut self, p: &PlacementSnapshot);
}

/// Callback that receives one image at a time during
/// [`FrameSource::visit_images`].
pub trait ImageVisitor {
    /// Visit one image. The pixel slice on `info` is borrowed and
    /// valid only for the call duration.
    fn image(&mut self, info: &ImageInfo<'_>);
}
