//! libghostty-vt implementation of [`FrameSource`].
//!
//! Wraps a [`Terminal`] and drives a `CellVisitor` over every cell in
//! the current VT snapshot. The only place in the workspace that
//! touches libghostty-vt's `RenderState` + `RowIterator` +
//! `CellIterator` dance; the renderer reads the resulting
//! [`CellView`] stream.

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

impl<'a> FrameSource for LibGhosttyFrameSource<'a> {
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
        let Ok(mut render_state) = RenderState::new() else {
            return;
        };
        let Ok(snapshot) = render_state.update(self.term.vt_mut()) else {
            return;
        };

        let Ok(mut row_iter_obj) = RowIterator::new() else {
            return;
        };
        let Ok(mut cell_iter_obj) = CellIterator::new() else {
            return;
        };
        let Ok(mut row_iter) = row_iter_obj.update(&snapshot) else {
            return;
        };

        let mut scratch = String::with_capacity(4);
        let mut row_idx: u16 = 0;

        while let Some(row) = row_iter.next() {
            let Ok(mut cell_iter) = cell_iter_obj.update(row) else {
                return;
            };
            let mut col_idx: u16 = 0;

            while let Some(cell) = cell_iter.next() {
                scratch.clear();
                if let Ok(graphs) = cell.graphemes() {
                    for c in graphs {
                        scratch.push(c);
                    }
                }

                let bg = resolve_bg(cell);
                let fg = resolve_fg(cell);

                visitor.cell(
                    row_idx,
                    col_idx,
                    CellView {
                        text: scratch.as_str(),
                        fg,
                        bg,
                    },
                );
                col_idx += 1;
            }
            row_idx += 1;
        }
    }
}

fn resolve_bg(cell: &CellIteration<'_, '_>) -> CellColor {
    if let Ok(Some(rgb)) = cell.bg_color() {
        return rgb_to_color(rgb);
    }
    if let Ok(style) = cell.style() {
        return style_color_to_cell_color(&style.bg_color);
    }
    CellColor::Default
}

fn resolve_fg(cell: &CellIteration<'_, '_>) -> CellColor {
    if let Ok(Some(rgb)) = cell.fg_color() {
        return rgb_to_color(rgb);
    }
    if let Ok(style) = cell.style() {
        return style_color_to_cell_color(&style.fg_color);
    }
    CellColor::Default
}

fn rgb_to_color(c: RgbColor) -> CellColor {
    CellColor::Rgb(c.r, c.g, c.b)
}

fn style_color_to_cell_color(sc: &style::StyleColor) -> CellColor {
    match sc {
        style::StyleColor::None => CellColor::Default,
        style::StyleColor::Palette(PaletteIndex(idx)) => CellColor::Palette(*idx),
        style::StyleColor::Rgb(rgb) => CellColor::Rgb(rgb.r, rgb.g, rgb.b),
    }
}
