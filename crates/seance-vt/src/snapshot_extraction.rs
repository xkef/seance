//! libghostty-vt snapshot extraction used by VT Core.

use libghostty_vt::RenderState;
use libghostty_vt::Terminal as VtTerminal;
use libghostty_vt::render::{CellIteration, CellIterator, CursorVisualStyle, Dirty, RowIterator};
use libghostty_vt::style::{self, PaletteIndex, RgbColor};
use libghostty_vt::terminal::Mode;

use crate::core::VtCoreError;
use crate::frame::{CellAttrs, CellColor, CursorInfo, CursorShape, DirtySnapshot};
use crate::kitty_graphics::{extract_kitty_graphics, is_placeholder_text};
use crate::modes::TerminalModes;
use crate::selection::GridPos;
use crate::snapshot::VtSnapshot;
use seance_protocol::MouseTracking;

/// Copy live libghostty mode flags into séance-owned input state.
pub(crate) fn terminal_modes(vt: &VtTerminal<'static, 'static>) -> TerminalModes {
    let mode = |m| vt.mode(m).unwrap_or(false);
    // Highest-precedence enabled mode wins. Apps typically only set
    // one at a time, but xterm semantics say the most permissive
    // tracking mode supersedes the others when more than one is on.
    let mouse_tracking = if mode(Mode::ANY_MOUSE) {
        MouseTracking::Any
    } else if mode(Mode::BUTTON_MOUSE) {
        MouseTracking::Button
    } else if mode(Mode::NORMAL_MOUSE) {
        MouseTracking::Normal
    } else if mode(Mode::X10_MOUSE) {
        MouseTracking::X10
    } else {
        MouseTracking::None
    };
    TerminalModes {
        cursor_keys: mode(Mode::DECCKM),
        mouse_tracking,
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
            for _col in 0..cols {
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
                out.push_cell(
                    &scratch,
                    resolve_fg(cell, style.as_ref()),
                    resolve_bg(cell, style.as_ref()),
                    cell_attrs(style.as_ref()),
                );
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
