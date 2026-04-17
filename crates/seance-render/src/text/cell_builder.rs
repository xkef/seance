//! Builds per-frame GPU records from a VT snapshot.
//!
//! Two passes:
//! 1. [`walk_grid`] — VT-aware, text-free. Walks the [`FrameSource`],
//!    resolves each cell's theme colors, emits a flat
//!    [`CellRequest`] list plus the background color buffer.
//! 2. [`shape_and_pack`] — text-aware, VT-free. Asks the
//!    [`TextBackend`] to shape each request, ensures each resulting
//!    glyph occupies an atlas slot, and emits [`CellText`] instance
//!    records keyed back to their grid cell.
//!
//! Either pass can be reworked without disturbing the other — the
//! planned path for swapping cosmic-text is entirely inside phase 2.

use std::collections::HashMap;

use rustc_hash::FxBuildHasher;
use seance_vt::{CellView, CellVisitor, FrameSource};

use super::atlas::{AtlasEntry, GlyphAtlas};
use super::backend::{GlyphFormat, GlyphId, ShapedGlyph, TextBackend};
use crate::theme::Theme;

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
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub grid_padding: [f32; 4],
    pub bg_color: [u8; 4],
    pub min_contrast: f32,
    pub cursor_pos: [u16; 2],
    pub cursor_color: [u8; 4],
    pub cursor_wide: bool,
}

/// One cell's shaping request produced by [`walk_grid`] and consumed
/// by [`shape_and_pack`]. Pre-resolved foreground color keeps the
/// shaping pass theme-free.
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
    /// Survives across frames; cleared only on font-size / scale
    /// changes via [`Self::reset_glyphs`].
    glyph_slots: HashMap<GlyphId, AtlasEntry, FxBuildHasher>,
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
    ) -> bool {
        let metrics = backend.metrics();
        let geom = geometry(
            source,
            metrics.cell_width,
            metrics.cell_height,
            surface_width,
            surface_height,
        );
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
            grid_cols: geom.grid_cols,
            grid_rows: geom.grid_rows,
            grid_padding: geom.grid_padding,
            bg_color: theme.bg,
            min_contrast: 1.0,
            cursor_pos: [cursor.pos.col, cursor.pos.row],
            cursor_color: theme.cursor,
            cursor_wide: cursor.wide,
        });
        true
    }

    /// Drop all atlas-cached glyphs. Call when font size or scale
    /// changes; the next frame will rebuild from scratch.
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
    let total_w = cols as f32 * cell_width;
    let total_h = rows as f32 * cell_height;
    let pad_x = ((surface_width as f32 - total_w) / 2.0).max(0.0);
    let pad_y = ((surface_height as f32 - total_h) / 2.0).max(0.0);
    FrameGeometry {
        cell_width,
        cell_height,
        grid_cols: cols,
        grid_rows: rows,
        grid_padding: [pad_x, pad_y, pad_x, pad_y],
    }
}

/// VT-aware, text-free pass. Walks the frame source, resolves each
/// cell's background color into `bg_cells` and records a
/// [`CellRequest`] per non-empty cell. No knowledge of the text
/// backend or the atlas.
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

/// Text-aware, VT-free pass. Shapes every request through the backend,
/// ensures each glyph has an atlas slot, and emits one
/// [`CellText`] per resulting glyph. No knowledge of the VT source.
fn shape_and_pack(
    requests: &[CellRequest],
    backend: &mut dyn TextBackend,
    atlas: &mut GlyphAtlas,
    glyph_slots: &mut HashMap<GlyphId, AtlasEntry, FxBuildHasher>,
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
                atlas_and_flags: if entry.is_color { 1 } else { 0 },
            });
        }
    }
}

fn ensure_glyph_slot(
    slots: &mut HashMap<GlyphId, AtlasEntry, FxBuildHasher>,
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

/// Visitor that resolves theme colors and queues shape requests.
/// Holds no backend/atlas references — stays on the VT side of the
/// wall.
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
        let cell_index = row as usize * self.cols as usize + col as usize;

        if let Some(rgb) = self.theme.resolve_color(&view.bg) {
            self.bg_cells[cell_index] = [rgb[0], rgb[1], rgb[2], 255];
        }

        if view.text.is_empty() {
            return;
        }

        let fg = match self.theme.resolve_color(&view.fg) {
            Some(rgb) => [rgb[0], rgb[1], rgb[2], 255],
            None => [self.theme.fg[0], self.theme.fg[1], self.theme.fg[2], 255],
        };

        self.requests.push(CellRequest {
            row,
            col,
            text: view.text.to_owned(),
            fg,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seance_vt::{CellColor, CellVisitor, CursorInfo, GridPos};

    /// Stub `FrameSource` that replays a fixed grid of `(text, fg, bg)`
    /// triples. Lets us unit-test `walk_grid` without a real VT.
    struct FakeFrame<'a> {
        cols: u16,
        rows: u16,
        cells: &'a [(&'a str, CellColor, CellColor)],
    }

    impl<'a> FrameSource for FakeFrame<'a> {
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
                let row = i / self.cols;
                let col = i % self.cols;
                visitor.cell(
                    row,
                    col,
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
        let theme = Theme::default();
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
