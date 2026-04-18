//! libghostty-vt implementation of [`FrameSource`].
//!
//! The only place that touches libghostty-vt's
//! `RenderState` / `RowIterator` / `CellIterator` dance.

use libghostty_vt::RenderState;
use libghostty_vt::render::{CellIteration, CellIterator, RowIterator};
use libghostty_vt::style::{self, PaletteIndex, RgbColor};

use crate::frame::{CellColor, CellView, CellVisitor, CursorInfo, FrameSource};
use crate::selection::GridPos;
use crate::terminal::Terminal;

pub struct LibGhosttyFrameSource<'a> {
    term: &'a mut Terminal,
}

impl<'a> LibGhosttyFrameSource<'a> {
    pub fn new(term: &'a mut Terminal) -> Self {
        Self { term }
    }
}

impl FrameSource for LibGhosttyFrameSource<'_> {
    fn grid_size(&mut self) -> (u16, u16) {
        let vt = self.term.vt_mut();
        (vt.cols().unwrap_or(80), vt.rows().unwrap_or(24))
    }

    fn cursor(&mut self) -> CursorInfo {
        let vt = self.term.vt_mut();
        CursorInfo {
            pos: GridPos {
                col: vt.cursor_x().unwrap_or(0),
                row: vt.cursor_y().unwrap_or(0),
            },
            visible: vt.is_cursor_visible().unwrap_or(true),
            wide: false,
        }
    }

    fn selection(&mut self) -> Option<(GridPos, GridPos)> {
        self.term.selection_range()
    }

    fn visit_cells(&mut self, visitor: &mut dyn CellVisitor) {
        let _ = walk(self.term, visitor);
    }
}

/// Walks the VT snapshot, invoking `visitor` on each cell.
/// Returns `None` if any libghostty-vt call fails (whole frame is dropped).
fn walk(term: &mut Terminal, visitor: &mut dyn CellVisitor) -> Option<()> {
    let mut render_state = RenderState::new().ok()?;
    let snapshot = render_state.update(term.vt_mut()).ok()?;
    let mut rows = RowIterator::new().ok()?;
    let mut cells = CellIterator::new().ok()?;
    let mut row_iter = rows.update(&snapshot).ok()?;

    let mut scratch = String::with_capacity(4);
    let mut row_idx: u16 = 0;
    while let Some(row) = row_iter.next() {
        let mut cell_iter = cells.update(row).ok()?;
        let mut col_idx: u16 = 0;
        while let Some(cell) = cell_iter.next() {
            scratch.clear();
            if let Ok(graphs) = cell.graphemes() {
                scratch.extend(graphs);
            }
            visitor.cell(
                row_idx,
                col_idx,
                CellView {
                    text: &scratch,
                    fg: resolve_fg(cell),
                    bg: resolve_bg(cell),
                },
            );
            col_idx += 1;
        }
        row_idx += 1;
    }
    Some(())
}

fn resolve_bg(cell: &CellIteration<'_, '_>) -> CellColor {
    if let Ok(Some(rgb)) = cell.bg_color() {
        return rgb_to_cell_color(rgb);
    }
    match cell.style() {
        Ok(style) => style_to_cell_color(&style.bg_color),
        Err(_) => CellColor::Default,
    }
}

fn resolve_fg(cell: &CellIteration<'_, '_>) -> CellColor {
    if let Ok(Some(rgb)) = cell.fg_color() {
        return rgb_to_cell_color(rgb);
    }
    match cell.style() {
        Ok(style) => style_to_cell_color(&style.fg_color),
        Err(_) => CellColor::Default,
    }
}

fn rgb_to_cell_color(c: RgbColor) -> CellColor {
    CellColor::Rgb(c.r, c.g, c.b)
}

fn style_to_cell_color(sc: &style::StyleColor) -> CellColor {
    match sc {
        style::StyleColor::None => CellColor::Default,
        style::StyleColor::Palette(PaletteIndex(idx)) => CellColor::Palette(*idx),
        style::StyleColor::Rgb(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}
