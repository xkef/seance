//! Build per-frame GPU records from a VT snapshot.
//!
//! Two passes:
//! 1. [`walk_grid`] — VT-aware, text-free. Resolves each cell's theme
//!    colors and emits a flat [`CellRequest`] list + the bg buffer.
//! 2. [`shape_and_pack`] — text-aware, VT-free. Shapes each request
//!    through the [`TextBackend`], ensures each glyph occupies an
//!    atlas slot, and emits one [`CellText`] per glyph.

use std::collections::HashMap;

use rustc_hash::FxBuildHasher;
use seance_config::Theme;
use seance_vt::{CellColor, CellView, CellVisitor, FrameSource};

use super::atlas::{AtlasEntry, GlyphAtlas};
use super::backend::{GlyphFormat, GlyphId, ShapedGlyph, TextBackend};

/// Resolve a VT-reported color into concrete RGB.
/// `None` means "use the theme default" (caller decides fg vs bg).
fn resolve_color(theme: &Theme, color: &CellColor) -> Option<[u8; 3]> {
    match *color {
        CellColor::Default => None,
        CellColor::Palette(idx) => Some(theme.palette[idx as usize]),
        CellColor::Rgb(r, g, b) => Some([r, g, b]),
    }
}

/// GPU instance record (32 bytes, matches the WGSL vertex buffer).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CellText {
    pub glyph_pos: [u32; 2],
    pub glyph_size: [u32; 2],
    pub bearings: [i16; 2],
    pub grid_pos: [u16; 2],
    pub color: [u8; 4],
    pub atlas_and_flags: u32,
}

const _: () = assert!(size_of::<CellText>() == 32);

pub struct FrameInfo {
    pub cell_width: f32,
    pub cell_height: f32,
    pub baseline: f32,
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub grid_padding: [f32; 4],
    pub bg_color: [u8; 4],
    pub min_contrast: f32,
    pub cursor_pos: [u16; 2],
    pub cursor_color: [u8; 4],
    pub cursor_wide: bool,
}

type GlyphSlots = HashMap<GlyphId, AtlasEntry, FxBuildHasher>;

/// One cell's shaping request produced by [`walk_grid`] and consumed
/// by [`shape_and_pack`].
struct CellRequest {
    row: u16,
    col: u16,
    text: String,
    fg: [u8; 4],
}

struct FrameGeometry {
    cell_width: f32,
    cell_height: f32,
    grid_cols: u16,
    grid_rows: u16,
    grid_padding: [f32; 4],
}

pub struct CellBuilder {
    atlas: GlyphAtlas,
    /// Stable map from backend-issued `GlyphId` to its atlas slot.
    /// Survives across frames; cleared by [`Self::reset_glyphs`].
    glyph_slots: GlyphSlots,
    bg_cells: Vec<[u8; 4]>,
    text_cells: Vec<CellText>,
    requests: Vec<CellRequest>,
    shape_scratch: Vec<ShapedGlyph>,
    last_frame: Option<FrameInfo>,
}

impl CellBuilder {
    pub fn new() -> Self {
        Self {
            atlas: GlyphAtlas::new(),
            glyph_slots: HashMap::with_hasher(FxBuildHasher),
            bg_cells: Vec::new(),
            text_cells: Vec::new(),
            requests: Vec::new(),
            shape_scratch: Vec::new(),
            last_frame: None,
        }
    }

    pub fn build_frame(
        &mut self,
        source: &mut dyn FrameSource,
        backend: &mut dyn TextBackend,
        surface_width: u32,
        surface_height: u32,
        theme: &Theme,
    ) {
        let (baseline, geom) = {
            let m = backend.metrics();
            let g = geometry(
                source,
                m.cell_width,
                m.cell_height,
                surface_width,
                surface_height,
            );
            (m.baseline, g)
        };
        let cursor = source.cursor();

        walk_grid(source, &geom, theme, &mut self.bg_cells, &mut self.requests);

        self.text_cells.clear();
        shape_and_pack(
            &self.requests,
            backend,
            &mut self.atlas,
            &mut self.glyph_slots,
            &mut self.shape_scratch,
            &mut self.text_cells,
        );

        self.atlas.clear_dirty();

        self.last_frame = Some(FrameInfo {
            cell_width: geom.cell_width,
            cell_height: geom.cell_height,
            baseline,
            grid_cols: geom.grid_cols,
            grid_rows: geom.grid_rows,
            grid_padding: geom.grid_padding,
            bg_color: theme.bg,
            min_contrast: 1.0,
            cursor_pos: [cursor.pos.col, cursor.pos.row],
            cursor_color: theme.cursor,
            cursor_wide: cursor.wide,
        });
    }

    /// Drop all atlas-cached glyphs. Call on font size / scale change.
    pub fn reset_glyphs(&mut self) {
        self.atlas.reset();
        self.glyph_slots.clear();
    }

    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    pub fn bg_cells(&self) -> &[[u8; 4]] {
        &self.bg_cells
    }

    pub fn text_cells(&self) -> &[CellText] {
        &self.text_cells
    }

    pub fn last_frame(&self) -> Option<&FrameInfo> {
        self.last_frame.as_ref()
    }
}

fn geometry(
    source: &mut dyn FrameSource,
    cell_width: f32,
    cell_height: f32,
    surface_width: u32,
    surface_height: u32,
) -> FrameGeometry {
    let (cols, rows) = source.grid_size();
    let pad_x = ((surface_width as f32 - cols as f32 * cell_width) / 2.0).max(0.0);
    let pad_y = ((surface_height as f32 - rows as f32 * cell_height) / 2.0).max(0.0);
    FrameGeometry {
        cell_width,
        cell_height,
        grid_cols: cols,
        grid_rows: rows,
        grid_padding: [pad_x, pad_y, pad_x, pad_y],
    }
}

/// VT-aware, text-free pass: resolve each cell's bg and queue a
/// shape request per non-empty cell.
fn walk_grid(
    source: &mut dyn FrameSource,
    geom: &FrameGeometry,
    theme: &Theme,
    bg_cells: &mut Vec<[u8; 4]>,
    requests: &mut Vec<CellRequest>,
) {
    bg_cells.clear();
    bg_cells.resize(geom.grid_cols as usize * geom.grid_rows as usize, theme.bg);
    requests.clear();

    let mut visitor = WalkVisitor {
        bg_cells,
        requests,
        theme,
        cols: geom.grid_cols,
        rows: geom.grid_rows,
    };
    source.visit_cells(&mut visitor);
}

/// Text-aware, VT-free pass: shape, cache glyphs, emit instance records.
fn shape_and_pack(
    requests: &[CellRequest],
    backend: &mut dyn TextBackend,
    atlas: &mut GlyphAtlas,
    glyph_slots: &mut GlyphSlots,
    shape_scratch: &mut Vec<ShapedGlyph>,
    out: &mut Vec<CellText>,
) {
    for req in requests {
        shape_scratch.clear();
        backend.shape_cell(&req.text, shape_scratch);
        for glyph in &*shape_scratch {
            let Some(entry) = ensure_glyph_slot(glyph_slots, atlas, backend, glyph.id) else {
                continue;
            };
            out.push(CellText {
                glyph_pos: entry.pos,
                glyph_size: entry.size,
                bearings: [entry.bearing_x as i16, entry.bearing_y as i16],
                grid_pos: [req.col, req.row],
                color: req.fg,
                atlas_and_flags: u32::from(entry.is_color),
            });
        }
    }
}

fn ensure_glyph_slot(
    slots: &mut GlyphSlots,
    atlas: &mut GlyphAtlas,
    backend: &mut dyn TextBackend,
    id: GlyphId,
) -> Option<AtlasEntry> {
    if let Some(e) = slots.get(&id) {
        return Some(*e);
    }
    let rast = backend.rasterize(id)?;
    let entry = atlas.insert(
        &rast.data,
        rast.width,
        rast.height,
        rast.bearing_x,
        rast.bearing_y,
        matches!(rast.format, GlyphFormat::Color),
    )?;
    slots.insert(id, entry);
    Some(entry)
}

/// Visitor: resolves theme colors, queues shape requests. Holds no
/// backend/atlas references — stays on the VT side of the wall.
struct WalkVisitor<'a> {
    bg_cells: &'a mut Vec<[u8; 4]>,
    requests: &'a mut Vec<CellRequest>,
    theme: &'a Theme,
    cols: u16,
    rows: u16,
}

impl CellVisitor for WalkVisitor<'_> {
    fn cell(&mut self, row: u16, col: u16, view: CellView<'_>) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let idx = row as usize * self.cols as usize + col as usize;

        if let Some(rgb) = resolve_color(self.theme, &view.bg) {
            self.bg_cells[idx] = [rgb[0], rgb[1], rgb[2], 255];
        }
        if view.text.is_empty() {
            return;
        }

        let fg_rgb = resolve_color(self.theme, &view.fg).unwrap_or(self.theme.fg);
        self.requests.push(CellRequest {
            row,
            col,
            text: view.text.to_owned(),
            fg: [fg_rgb[0], fg_rgb[1], fg_rgb[2], 255],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seance_vt::{CellColor, CellVisitor, CursorInfo, GridPos};

    /// Stub `FrameSource` replaying a fixed `(text, fg, bg)` grid.
    struct FakeFrame<'a> {
        cols: u16,
        rows: u16,
        cells: &'a [(&'a str, CellColor, CellColor)],
    }

    impl FrameSource for FakeFrame<'_> {
        fn grid_size(&mut self) -> (u16, u16) {
            (self.cols, self.rows)
        }
        fn cursor(&mut self) -> CursorInfo {
            CursorInfo::default()
        }
        fn selection(&mut self) -> Option<(GridPos, GridPos)> {
            None
        }
        fn visit_cells(&mut self, visitor: &mut dyn CellVisitor) {
            for (i, (text, fg, bg)) in self.cells.iter().enumerate() {
                let i = i as u16;
                visitor.cell(
                    i / self.cols,
                    i % self.cols,
                    CellView {
                        text,
                        fg: *fg,
                        bg: *bg,
                    },
                );
            }
        }
    }

    #[test]
    fn walk_grid_emits_one_request_per_non_empty_cell() {
        let theme = Theme::blank();
        let cells = [
            ("A", CellColor::Default, CellColor::Default),
            ("", CellColor::Default, CellColor::Default),
            ("B", CellColor::Rgb(10, 20, 30), CellColor::Default),
        ];
        let mut source = FakeFrame {
            cols: 3,
            rows: 1,
            cells: &cells,
        };
        let geom = FrameGeometry {
            cell_width: 10.0,
            cell_height: 20.0,
            grid_cols: 3,
            grid_rows: 1,
            grid_padding: [0.0; 4],
        };
        let mut bg_cells = Vec::new();
        let mut requests = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut requests);

        assert_eq!(bg_cells.len(), 3);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].text, "A");
        assert_eq!(requests[0].col, 0);
        assert_eq!(requests[1].text, "B");
        assert_eq!(requests[1].col, 2);
        assert_eq!(requests[1].fg, [10, 20, 30, 255]);
    }
}
