//! The VT / render wall.
//!
//! The renderer consumes a grid snapshot through [`FrameSource`]; any
//! VT emulator can adapt to this trait without the renderer knowing
//! its type. Today the sole implementor lives in `seance-vt` and
//! wraps libghostty-vt. Swapping to a different VT engine would be
//! local to that adapter.

use crate::selection::GridPos;

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
}

pub trait CellVisitor {
    fn cell(&mut self, row: u16, col: u16, view: CellView<'_>);
}
