//! Layer 4 frame dump.
//!
//! Walks a [`FrameSource`] and returns a deterministic, LLM-readable
//! string: a rounded-corner box showing the visual grid, followed by
//! annotations for cells with non-default colors, plus cursor and
//! selection state.
//!
//! The box uses rounded-corner glyphs (╭╮╰╯) so fixture content that
//! uses square box-drawing (┌┐└┘) doesn't collide with the frame.

use std::fmt::Write;

use seance_vt::{CellColor, CellView, CellVisitor, FrameSource};
use unicode_width::UnicodeWidthStr;

/// Build the L4 text dump for `source`.
pub fn dump_frame(source: &mut dyn FrameSource) -> String {
    let (cols, rows) = source.grid_size();
    let cursor = source.cursor();
    let selection = source.selection();

    let mut collector = Collector {
        cells: Vec::with_capacity(usize::from(cols) * usize::from(rows)),
    };
    source.visit_cells(&mut collector);

    let grid = layout_grid(&collector.cells, cols, rows);

    let mut out = String::new();
    render_box(&mut out, &grid, cols);
    render_annotations(&mut out, &collector.cells);
    render_cursor(&mut out, cursor.pos.col, cursor.pos.row, cursor.visible);
    render_selection(&mut out, selection);
    out
}

struct Collector {
    cells: Vec<CellSnapshot>,
}

struct CellSnapshot {
    row: u16,
    col: u16,
    text: String,
    fg: CellColor,
    bg: CellColor,
}

impl CellVisitor for Collector {
    fn cell(&mut self, row: u16, col: u16, view: CellView<'_>) {
        self.cells.push(CellSnapshot {
            row,
            col,
            text: view.text.to_string(),
            fg: view.fg,
            bg: view.bg,
        });
    }
}

fn layout_grid(cells: &[CellSnapshot], cols: u16, rows: u16) -> Vec<Vec<Option<&CellSnapshot>>> {
    let mut grid: Vec<Vec<Option<&CellSnapshot>>> = (0..usize::from(rows))
        .map(|_| vec![None; usize::from(cols)])
        .collect();
    for cell in cells {
        let r = usize::from(cell.row);
        let c = usize::from(cell.col);
        if r < grid.len() && c < usize::from(cols) {
            grid[r][c] = Some(cell);
        }
    }
    grid
}

fn render_box(out: &mut String, grid: &[Vec<Option<&CellSnapshot>>], cols: u16) {
    let border: String = "─".repeat(usize::from(cols));
    let _ = writeln!(out, "╭{border}╮");
    for row_cells in grid {
        out.push('│');
        let mut col = 0usize;
        let width = usize::from(cols);
        while col < width {
            match row_cells[col] {
                Some(cell) if !cell.text.is_empty() => {
                    out.push_str(&cell.text);
                    let w = UnicodeWidthStr::width(cell.text.as_str()).max(1);
                    col += w;
                }
                _ => {
                    out.push(' ');
                    col += 1;
                }
            }
        }
        out.push('│');
        out.push('\n');
    }
    let _ = writeln!(out, "╰{border}╯");
}

fn render_annotations(out: &mut String, cells: &[CellSnapshot]) {
    out.push_str("cells:\n");
    let mut any = false;
    for cell in cells {
        if is_annotation_worthy(cell) {
            let _ = writeln!(
                out,
                "  ({},{}) {:?} fg={} bg={}",
                cell.row,
                cell.col,
                cell.text,
                fmt_color(cell.fg),
                fmt_color(cell.bg),
            );
            any = true;
        }
    }
    if !any {
        out.push_str("  (none)\n");
    }
}

fn render_cursor(out: &mut String, col: u16, row: u16, visible: bool) {
    let _ = writeln!(out, "cursor: ({row},{col}) visible={visible}");
}

fn render_selection(out: &mut String, selection: Option<(seance_vt::GridPos, seance_vt::GridPos)>) {
    match selection {
        Some((start, end)) => {
            let _ = writeln!(
                out,
                "selection: ({},{})→({},{})",
                start.row, start.col, end.row, end.col,
            );
        }
        None => out.push_str("selection: none\n"),
    }
}

fn is_annotation_worthy(cell: &CellSnapshot) -> bool {
    !matches!(cell.fg, CellColor::Default) || !matches!(cell.bg, CellColor::Default)
}

fn fmt_color(c: CellColor) -> String {
    match c {
        CellColor::Default => "default".to_string(),
        CellColor::Palette(p) => format!("pal:{p}"),
        CellColor::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}
