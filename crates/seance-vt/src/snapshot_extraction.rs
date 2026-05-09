//! libghostty-vt snapshot extraction used by VT Core.

use libghostty_vt::RenderState;
use libghostty_vt::Terminal as VtTerminal;
use libghostty_vt::error::Error as LibghosttyError;
use libghostty_vt::render::{CellIteration, CellIterator, CursorVisualStyle, Dirty, RowIterator};
use libghostty_vt::style::{self, PaletteIndex, RgbColor};
use libghostty_vt::terminal::{Mode, Point, PointCoordinate};

use crate::core::VtCoreError;
use crate::frame::{CellAttrs, CellColor, CursorInfo, CursorShape, DirtySnapshot};
use crate::kitty_graphics::{extract_kitty_graphics, is_placeholder_text};
use crate::modes::TerminalModes;
use crate::selection::GridPos;
use crate::snapshot::VtSnapshot;

/// Copy live libghostty mode flags into séance-owned input state.
pub(crate) fn terminal_modes(vt: &VtTerminal<'static, 'static>) -> TerminalModes {
    let mode = |m| vt.mode(m).unwrap_or(false);
    TerminalModes {
        cursor_keys: mode(Mode::DECCKM),
        mouse_tracking: vt.is_mouse_tracking().unwrap_or(false),
        mouse_format_sgr: mode(Mode::SGR_MOUSE),
        bracketed_paste: mode(Mode::BRACKETED_PASTE),
    }
}

pub(crate) struct SnapshotExtraction {
    pub(crate) snapshot: VtSnapshot,
    pub(crate) dirty_delta: DirtySnapshot,
}

pub(crate) fn extract_snapshot(
    vt: &mut VtTerminal<'static, 'static>,
    render_state: &mut RenderState<'static>,
    cell_width_px: u32,
    cell_height_px: u32,
) -> Result<SnapshotExtraction, VtCoreError> {
    let cols = vt.cols().unwrap_or(80);
    let rows = vt.rows().unwrap_or(24);
    let modes = terminal_modes(vt);

    let mut out = VtSnapshot::empty(cols, rows);
    out.modes = modes;

    let dirty_delta;
    // Cells whose URL we still need to query, deferred until the
    // render-state borrow is released so we can re-borrow `vt` for
    // `grid_ref`. Stores `(row, col, cell_index_in_out)`.
    let mut pending_links: Vec<(u32, u16, usize)> = Vec::new();
    {
        let render_snapshot = render_state
            .update(vt)
            .map_err(|_| VtCoreError::libghostty("render state update"))?;
        let global_dirty = render_snapshot.dirty().ok();
        let mut dirty_rows = Vec::new();

        let visible = render_snapshot.cursor_visible().unwrap_or(true);
        let pos =
            render_snapshot
                .cursor_viewport()
                .ok()
                .flatten()
                .map_or(GridPos::default(), |vp| GridPos {
                    col: vp.x,
                    row: vp.y,
                });
        let shape = render_snapshot
            .cursor_visual_style()
            .ok()
            .and_then(map_cursor_shape);
        out.cursor = CursorInfo {
            pos,
            visible,
            wide: false,
            shape,
        };

        let mut rows_iter =
            RowIterator::new().map_err(|_| VtCoreError::libghostty("row iterator new"))?;
        let mut cells_iter =
            CellIterator::new().map_err(|_| VtCoreError::libghostty("cell iterator new"))?;
        let mut row_iter = rows_iter
            .update(&render_snapshot)
            .map_err(|_| VtCoreError::libghostty("row iterator update"))?;
        let mut scratch = String::with_capacity(4);

        for row_idx in 0..rows {
            let Some(row) = row_iter.next() else {
                for _ in 0..cols {
                    out.push_empty_cell();
                }
                continue;
            };
            match global_dirty {
                Some(Dirty::Partial) if row.dirty().unwrap_or(true) => dirty_rows.push(row_idx),
                None if row.dirty().unwrap_or(false) => dirty_rows.push(row_idx),
                _ => {}
            }
            let mut cell_iter = cells_iter
                .update(row)
                .map_err(|_| VtCoreError::libghostty("cell iterator update"))?;
            // Skip the per-cell hyperlink probe entirely when the row
            // has no OSC 8 cells. libghostty's flag may have false
            // positives so the per-cell check still gates URL lookup.
            let row_has_links = row
                .raw_row()
                .ok()
                .and_then(|r| r.has_hyperlink().ok())
                .unwrap_or(false);
            for col in 0..cols {
                let Some(cell) = cell_iter.next() else {
                    out.push_empty_cell();
                    continue;
                };
                scratch.clear();
                if let Ok(graphs) = cell.graphemes() {
                    scratch.extend(graphs);
                }
                if is_placeholder_text(&scratch) {
                    scratch.clear();
                }
                let style = cell.style().ok();
                let cell_has_link = row_has_links
                    && cell
                        .raw_cell()
                        .ok()
                        .and_then(|c| c.has_hyperlink().ok())
                        .unwrap_or(false);
                out.push_cell(
                    &scratch,
                    resolve_fg(cell, style.as_ref()),
                    resolve_bg(cell, style.as_ref()),
                    cell_attrs(style.as_ref()),
                );
                if cell_has_link {
                    pending_links.push((u32::from(row_idx), col, out.cells.len() - 1));
                }
            }
            row.set_dirty(false)
                .map_err(|_| VtCoreError::libghostty("clear row dirty"))?;
        }

        dirty_delta = match global_dirty {
            Some(Dirty::Clean) => DirtySnapshot::Clean,
            Some(Dirty::Full) => DirtySnapshot::Full,
            Some(Dirty::Partial) | None => {
                if dirty_rows.is_empty() {
                    DirtySnapshot::Clean
                } else {
                    DirtySnapshot::Partial(dirty_rows)
                }
            }
        };
        render_snapshot
            .set_dirty(Dirty::Clean)
            .map_err(|_| VtCoreError::libghostty("clear render dirty"))?;
    }

    if !pending_links.is_empty() {
        resolve_pending_hyperlinks(vt, &pending_links, &mut out);
    }

    let graphics = extract_kitty_graphics(vt, cell_width_px, cell_height_px);
    out.placements = graphics.placements;
    out.images = graphics.images;

    Ok(SnapshotExtraction {
        snapshot: out,
        dirty_delta,
    })
}

fn resolve_bg(cell: &CellIteration<'_, '_>, style: Option<&style::Style>) -> CellColor {
    if let Ok(Some(rgb)) = cell.bg_color() {
        return rgb_to_cell_color(rgb);
    }
    style.map_or(CellColor::Default, |style| {
        style_to_cell_color(&style.bg_color)
    })
}

fn resolve_fg(cell: &CellIteration<'_, '_>, style: Option<&style::Style>) -> CellColor {
    if let Ok(Some(rgb)) = cell.fg_color() {
        return rgb_to_cell_color(rgb);
    }
    style.map_or(CellColor::Default, |style| {
        style_to_cell_color(&style.fg_color)
    })
}

fn cell_attrs(style: Option<&style::Style>) -> CellAttrs {
    CellAttrs {
        bold: style.is_some_and(|style| style.bold),
        italic: style.is_some_and(|style| style.italic),
        faint: style.is_some_and(|style| style.faint),
        inverse: style.is_some_and(|style| style.inverse),
        invisible: style.is_some_and(|style| style.invisible),
    }
}

/// Walk every cell whose row/cell hyperlink flags fired and copy its OSC 8
/// URL into `out.hyperlinks`, deduping via [`VtSnapshot::intern_hyperlink`].
/// Run after the `RenderState` borrow is released so `vt.grid_ref` can
/// re-borrow the terminal.
fn resolve_pending_hyperlinks(
    vt: &VtTerminal<'static, 'static>,
    pending: &[(u32, u16, usize)],
    out: &mut VtSnapshot,
) {
    let mut buf: Vec<u8> = vec![0; 256];
    for &(row, col, cell_idx) in pending {
        let point = Point::Viewport(PointCoordinate { x: col, y: row });
        let Ok(grid_ref) = vt.grid_ref(point) else {
            continue;
        };
        let written = match grid_ref.hyperlink_uri(&mut buf) {
            Ok(n) => n,
            Err(LibghosttyError::OutOfSpace { required }) => {
                buf.resize(required, 0);
                match grid_ref.hyperlink_uri(&mut buf) {
                    Ok(n) => n,
                    Err(_) => continue,
                }
            }
            Err(_) => continue,
        };
        if written == 0 {
            continue;
        }
        let Ok(url) = std::str::from_utf8(&buf[..written]) else {
            continue;
        };
        let idx = out.intern_hyperlink(url);
        if let Some(cell) = out.cells.get_mut(cell_idx) {
            cell.hyperlink_idx = idx;
        }
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

// `BlockHollow` is ghostty's unfocused-window rendering, not a DECSCUSR
// request — collapse into `Block`. Window-focus hollow cursors are a
// separate feature tracked elsewhere. Unknown future variants (the
// enum is `#[non_exhaustive]`) return `None` so the app falls back to
// the user's configured shape.
fn map_cursor_shape(s: CursorVisualStyle) -> Option<CursorShape> {
    match s {
        CursorVisualStyle::Bar => Some(CursorShape::Bar),
        CursorVisualStyle::Block | CursorVisualStyle::BlockHollow => Some(CursorShape::Block),
        CursorVisualStyle::Underline => Some(CursorShape::Underline),
        _ => None,
    }
}
