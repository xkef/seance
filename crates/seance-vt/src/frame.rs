//! The VT / render wall.
//!
//! The renderer consumes a grid snapshot through [`FrameSource`]; any
//! VT emulator can adapt to this trait without the renderer knowing
//! its type. Today the sole implementor lives in `seance-vt` and
//! wraps libghostty-vt. Swapping to a different VT engine would be
//! local to that adapter.

use crate::selection::GridPos;

/// Z-layer a kitty graphics placement belongs to.
///
/// Ghostty partitions placements by their integer `z` value. Using the
/// filter at iteration time lets the renderer record one draw list per
/// layer without re-sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlacementLayer {
    /// `z < i32::MIN / 2` — below cell background.
    BelowBg,
    /// `i32::MIN / 2 ≤ z < 0` — above background, below text.
    BelowText,
    /// `z ≥ 0` — above text.
    AboveText,
}

/// One kitty graphics placement visible in the current viewport.
///
/// All fields use `libghostty-vt`'s resolved values: `viewport_col/row`
/// may be negative when a placement has partially scrolled off the top;
/// `pixel_width/height` already account for aspect ratio and cell
/// dimensions; `source_*` is clamped to image bounds. `image_width/
/// height` are reported here so the renderer can compute source UVs
/// without a separate cache lookup.
#[derive(Debug, Clone, Copy)]
pub struct PlacementSnapshot {
    pub image_id: u32,
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

/// One image's pixel payload, referenced by placements by `image_id`.
///
/// `rgba` is always 8-bit tightly-packed RGBA; non-RGBA source formats
/// are expanded by the VT adapter before emission. The slice is valid
/// only for the duration of the [`ImageVisitor::image`] call.
pub struct ImageInfo<'a> {
    pub image_id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

/// A color slot in a terminal cell. Resolved by the renderer using
/// its theme — the VT layer reports what the VT sees, not pixels.
#[derive(Debug, Clone, Copy)]
pub enum CellColor {
    /// Use the theme's default foreground / background.
    Default,
    /// Index into the 256-color palette.
    Palette(u8),
    /// Direct RGB color set by the VT (truecolor escapes).
    Rgb(u8, u8, u8),
}

/// A cell's renderable content at a point in time.
///
/// `text` is backed by scratch storage in the adapter; it is valid
/// only for the duration of one [`CellVisitor::cell`] call.
pub struct CellView<'a> {
    pub text: &'a str,
    pub fg: CellColor,
    pub bg: CellColor,
}

/// Cursor pose the renderer needs to place the block/underline/bar.
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorInfo {
    pub pos: GridPos,
    pub visible: bool,
    pub wide: bool,
}

/// A snapshot of the VT grid the renderer walks to build one frame.
pub trait FrameSource {
    /// (columns, rows) of the current grid.
    fn grid_size(&mut self) -> (u16, u16);

    /// VT cursor position and visibility.
    fn cursor(&mut self) -> CursorInfo;

    /// Active selection range in grid coordinates, if any.
    fn selection(&mut self) -> Option<(GridPos, GridPos)>;

    /// Drive a visitor over every cell. The adapter is responsible for
    /// issuing calls in row-major order and clamping to `grid_size`.
    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor);

    /// Emit kitty graphics placements in the requested z-layer.
    ///
    /// Implementations filter by layer and skip placements outside the
    /// viewport. Virtual (unicode placeholder) placements are skipped in
    /// the v1 path. Default impl emits nothing for adapters without
    /// graphics support.
    fn visit_placements(
        &mut self,
        _layer: PlacementLayer,
        _visitor: &mut dyn PlacementVisitor,
    ) {
    }

    /// Emit pixel payloads for images referenced by visible placements.
    ///
    /// The adapter dedupes by `image_id` and expands non-RGBA formats
    /// to RGBA8 before calling the visitor. Default impl emits nothing.
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
