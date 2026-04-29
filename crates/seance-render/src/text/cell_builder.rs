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
use seance_vt::{CellColor, CellView, CellVisitor, DirtySnapshot, FrameSource};

use super::atlas::{AtlasEntry, GlyphAtlas};
use super::backend::{FontAttrs, GlyphFormat, GlyphId, ShapedGlyph, TextBackend};
use super::shape_cache::ShapeCache;

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
    pub cursor_visible: bool,
    pub cursor_color: [u8; 4],
    pub cursor_wide: bool,
}

pub struct BuildFrameConfig<'a> {
    pub surface_width: u32,
    pub surface_height: u32,
    pub window_padding: [u16; 2],
    pub theme: &'a Theme,
    pub bg_color: [u8; 4],
    pub min_contrast: f32,
}

type GlyphSlots = HashMap<GlyphId, AtlasEntry, FxBuildHasher>;

/// One cell's shaping request produced by [`walk_grid`] and consumed
/// by [`shape_and_pack`].
struct CellRequest {
    row: u16,
    col: u16,
    text: String,
    fg: [u8; 4],
    font_attrs: FontAttrs,
}

struct FrameGeometry {
    cell_width: f32,
    cell_height: f32,
    grid_cols: u16,
    grid_rows: u16,
    grid_padding: [f32; 4],
}

const FAINT_ALPHA: u8 = 128;

pub struct CellBuilder {
    atlas: GlyphAtlas,
    /// Stable map from backend-issued `GlyphId` to its atlas slot.
    /// Survives across frames; cleared by [`Self::reset_glyphs`].
    glyph_slots: GlyphSlots,
    /// Memoized `shape_cell` output keyed by `(font flags, text)`.
    /// Cleared alongside `glyph_slots` because both are tied to the
    /// active font size / scale.
    shape_cache: ShapeCache,
    bg_cells: Vec<[u8; 4]>,
    text_cells: Vec<CellText>,
    requests: Vec<CellRequest>,
    shape_scratch: Vec<ShapedGlyph>,
    last_frame: Option<FrameInfo>,
    last_dirty: DirtySnapshot,
}

impl CellBuilder {
    pub fn new() -> Self {
        Self {
            atlas: GlyphAtlas::new(),
            glyph_slots: HashMap::with_hasher(FxBuildHasher),
            shape_cache: ShapeCache::new(),
            bg_cells: Vec::new(),
            text_cells: Vec::new(),
            requests: Vec::new(),
            shape_scratch: Vec::new(),
            last_frame: None,
            // First frame must be a full upload — there's nothing on the
            // GPU yet for a `Partial` write to layer onto.
            last_dirty: DirtySnapshot::Full,
        }
    }

    pub fn build_frame(
        &mut self,
        source: &mut dyn FrameSource,
        backend: &mut dyn TextBackend,
        config: BuildFrameConfig<'_>,
    ) {
        // Sample the dirty set first so the rest of the build can mutate
        // the source freely. The snapshot is owned by the builder; the
        // matching `clear_dirty` call below acknowledges it on the VT
        // side so the next frame reports only post-snapshot changes.
        self.last_dirty = source.dirty_rows();
        source.clear_dirty();

        let (baseline, geom) = {
            let m = backend.metrics();
            let g = geometry(
                source,
                m.cell_width,
                m.cell_height,
                config.surface_width,
                config.surface_height,
                config.window_padding,
            );
            (m.baseline, g)
        };
        let cursor = source.cursor();

        walk_grid(
            source,
            &geom,
            config.theme,
            &mut self.bg_cells,
            &mut self.requests,
        );

        self.text_cells.clear();
        shape_and_pack(
            &self.requests,
            backend,
            &mut self.atlas,
            &mut self.glyph_slots,
            &mut self.shape_cache,
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
            bg_color: config.bg_color,
            min_contrast: config.min_contrast,
            cursor_pos: [cursor.pos.col, cursor.pos.row],
            cursor_visible: cursor.visible,
            cursor_color: config.theme.cursor,
            cursor_wide: cursor.wide,
        });
    }

    /// Drop all atlas-cached glyphs and shape cache entries. Call on
    /// font size / scale change. A future font-family-change API
    /// must also call this — the cache key omits the family because
    /// it is implicit backend state, so swapping families without a
    /// reset would return stale glyph IDs.
    pub fn reset_glyphs(&mut self) {
        self.atlas.reset();
        self.glyph_slots.clear();
        self.shape_cache.clear();
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

    pub fn last_dirty(&self) -> &DirtySnapshot {
        &self.last_dirty
    }

    #[cfg(test)]
    pub(crate) fn shape_cache_stats(&self) -> super::shape_cache::CacheStats {
        *self.shape_cache.stats()
    }
}

fn geometry(
    source: &mut dyn FrameSource,
    cell_width: f32,
    cell_height: f32,
    surface_width: u32,
    surface_height: u32,
    window_padding: [u16; 2],
) -> FrameGeometry {
    let (cols, rows) = source.grid_size();
    // User-configured padding anchors the grid at `(padding_x, padding_y)`.
    // Any residual between the grid pixel extent and the surface is left on
    // the right/bottom and filled by the fullscreen bg pass — matches
    // Ghostty/Alacritty semantics (padding = minimum gutter, not centering).
    let user_pad_x = f32::from(window_padding[0]);
    let user_pad_y = f32::from(window_padding[1]);
    let pad_x = user_pad_x
        .min((surface_width as f32 - cols as f32 * cell_width).max(0.0))
        .max(0.0);
    let pad_y = user_pad_y
        .min((surface_height as f32 - rows as f32 * cell_height).max(0.0))
        .max(0.0);
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
    bg_cells.resize(
        geom.grid_cols as usize * geom.grid_rows as usize,
        [0, 0, 0, 0],
    );
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

/// Text-aware, VT-free pass: shape (with cache), cache glyphs, emit
/// instance records.
fn shape_and_pack(
    requests: &[CellRequest],
    backend: &mut dyn TextBackend,
    atlas: &mut GlyphAtlas,
    glyph_slots: &mut GlyphSlots,
    shape_cache: &mut ShapeCache,
    shape_scratch: &mut Vec<ShapedGlyph>,
    out: &mut Vec<CellText>,
) {
    for req in requests {
        shape_with_cache(
            shape_cache,
            backend,
            &req.text,
            req.font_attrs,
            shape_scratch,
        );
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

/// Run `text` through the shape cache, falling through to `backend`
/// on miss and inserting the result. Always leaves `scratch` holding
/// the shaped glyphs for the caller.
fn shape_with_cache(
    cache: &mut ShapeCache,
    backend: &mut dyn TextBackend,
    text: &str,
    attrs: FontAttrs,
    scratch: &mut Vec<ShapedGlyph>,
) {
    scratch.clear();
    if cache.lookup_into(text, attrs, scratch) {
        return;
    }
    backend.shape_cell(text, attrs, scratch);
    cache.insert(text, attrs, scratch);
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

        let theme_bg = [self.theme.bg[0], self.theme.bg[1], self.theme.bg[2]];
        let mut fg_rgb = resolve_color(self.theme, &view.fg).unwrap_or(self.theme.fg);
        let mut bg_rgb = resolve_color(self.theme, &view.bg).unwrap_or(theme_bg);
        if view.attrs.inverse {
            std::mem::swap(&mut fg_rgb, &mut bg_rgb);
        }
        if bg_rgb != theme_bg {
            self.bg_cells[idx] = [bg_rgb[0], bg_rgb[1], bg_rgb[2], 255];
        }
        if view.text.is_empty() || view.attrs.invisible {
            return;
        }

        let alpha = if view.attrs.faint { FAINT_ALPHA } else { 255 };
        self.requests.push(CellRequest {
            row,
            col,
            text: view.text.to_owned(),
            fg: [fg_rgb[0], fg_rgb[1], fg_rgb[2], alpha],
            font_attrs: FontAttrs {
                bold: view.attrs.bold,
                italic: view.attrs.italic,
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::backend::{
        CellMetrics, FontAttrs, GlyphId, RasterizedGlyph, ShapedGlyph, TextBackend,
    };
    use seance_vt::{CellAttrs, CellColor, CellVisitor, CursorInfo, GridPos};

    struct FakeFrame<'a> {
        cols: u16,
        rows: u16,
        cells: &'a [(&'a str, CellColor, CellColor, CellAttrs)],
        dirty: DirtySnapshot,
        clear_count: u32,
    }

    impl<'a> FakeFrame<'a> {
        fn new(
            cols: u16,
            rows: u16,
            cells: &'a [(&'a str, CellColor, CellColor, CellAttrs)],
        ) -> Self {
            Self {
                cols,
                rows,
                cells,
                dirty: DirtySnapshot::Full,
                clear_count: 0,
            }
        }
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
            for (i, (text, fg, bg, attrs)) in self.cells.iter().enumerate() {
                let i = i as u16;
                visitor.cell(
                    i / self.cols,
                    i % self.cols,
                    CellView {
                        text,
                        fg: *fg,
                        bg: *bg,
                        attrs: *attrs,
                    },
                );
            }
        }
        fn dirty_rows(&mut self) -> DirtySnapshot {
            self.dirty.clone()
        }
        fn clear_dirty(&mut self) {
            self.clear_count += 1;
            self.dirty = DirtySnapshot::Clean;
        }
    }

    struct StubBackend {
        metrics: CellMetrics,
        /// Tracks how many times `shape_cell` was invoked. The cache
        /// integration tests assert this hits zero on a warm second
        /// frame — that's the whole point of the cache.
        shape_calls: u32,
    }

    impl StubBackend {
        fn new() -> Self {
            Self {
                metrics: CellMetrics {
                    cell_width: 10.0,
                    cell_height: 20.0,
                    baseline: 16.0,
                },
                shape_calls: 0,
            }
        }
    }

    impl TextBackend for StubBackend {
        fn metrics(&self) -> &CellMetrics {
            &self.metrics
        }
        fn set_font_size(&mut self, _points: f32) {}
        fn set_scale(&mut self, _scale: f64) {}
        fn set_adjust_cell_height(&mut self, _value: Option<&str>) {}
        fn shape_cell(&mut self, text: &str, _attrs: FontAttrs, out: &mut Vec<ShapedGlyph>) {
            self.shape_calls += 1;
            // Deterministic single-glyph result keyed off the first
            // codepoint so cache round-trips are observable.
            if let Some(c) = text.chars().next() {
                out.push(ShapedGlyph {
                    id: GlyphId(u64::from(u32::from(c))),
                });
            }
        }
        fn rasterize(&mut self, _glyph: GlyphId) -> Option<RasterizedGlyph> {
            None
        }
    }

    fn build_config(theme: &Theme) -> BuildFrameConfig<'_> {
        BuildFrameConfig {
            surface_width: 100,
            surface_height: 100,
            window_padding: [0, 0],
            theme,
            bg_color: [0, 0, 0, 255],
            min_contrast: 1.0,
        }
    }

    #[test]
    fn build_frame_captures_dirty_snapshot_and_acknowledges_source() {
        let theme = Theme::blank();
        let cells = [(
            "A",
            CellColor::Default,
            CellColor::Default,
            CellAttrs::default(),
        )];
        let mut source = FakeFrame::new(1, 1, &cells);
        source.dirty = DirtySnapshot::Partial(vec![0]);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));

        assert_eq!(*builder.last_dirty(), DirtySnapshot::Partial(vec![0]));
        assert_eq!(source.clear_count, 1);
        // Sticky-clear simulation: the second build sees Clean because
        // the fake reset itself in clear_dirty.
        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(*builder.last_dirty(), DirtySnapshot::Clean);
        assert_eq!(source.clear_count, 2);
    }

    #[test]
    fn build_frame_first_call_defaults_last_dirty_to_full_until_sampled() {
        // Before any build_frame, last_dirty is Full so the first GPU
        // upload is the full path. After build_frame, last_dirty mirrors
        // whatever the source reported.
        let builder = CellBuilder::new();
        assert_eq!(*builder.last_dirty(), DirtySnapshot::Full);
    }

    #[test]
    fn walk_grid_emits_one_request_per_non_empty_cell() {
        let theme = Theme::blank();
        let cells = [
            (
                "A",
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            ),
            (
                "",
                CellColor::Default,
                CellColor::Default,
                CellAttrs::default(),
            ),
            (
                "B",
                CellColor::Rgb(10, 20, 30),
                CellColor::Default,
                CellAttrs::default(),
            ),
        ];
        let mut source = FakeFrame::new(3, 1, &cells);
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
        assert_eq!(bg_cells[0], [0, 0, 0, 0]);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].text, "A");
        assert_eq!(requests[0].col, 0);
        assert_eq!(requests[1].text, "B");
        assert_eq!(requests[1].col, 2);
        assert_eq!(requests[1].fg, [10, 20, 30, 255]);
        assert_eq!(requests[1].font_attrs, FontAttrs::default());
    }

    #[test]
    fn walk_grid_applies_faint_alpha() {
        let theme = Theme::blank();
        let cells = [(
            "dim",
            CellColor::Palette(6),
            CellColor::Default,
            CellAttrs {
                faint: true,
                ..CellAttrs::default()
            },
        )];
        let mut source = FakeFrame::new(1, 1, &cells);
        let geom = FrameGeometry {
            cell_width: 10.0,
            cell_height: 20.0,
            grid_cols: 1,
            grid_rows: 1,
            grid_padding: [0.0; 4],
        };
        let mut bg_cells = Vec::new();
        let mut requests = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].fg, [0, 205, 205, FAINT_ALPHA]);
    }

    #[test]
    fn walk_grid_swaps_fg_and_bg_for_inverse_cells() {
        let theme = Theme::blank();
        let cells = [(
            "inv",
            CellColor::Palette(1),
            CellColor::Default,
            CellAttrs {
                inverse: true,
                ..CellAttrs::default()
            },
        )];
        let mut source = FakeFrame::new(1, 1, &cells);
        let geom = FrameGeometry {
            cell_width: 10.0,
            cell_height: 20.0,
            grid_cols: 1,
            grid_rows: 1,
            grid_padding: [0.0; 4],
        };
        let mut bg_cells = Vec::new();
        let mut requests = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut requests);

        assert_eq!(bg_cells[0], [205, 0, 0, 255]);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].fg, [0, 0, 0, 255]);
    }

    #[test]
    fn walk_grid_skips_invisible_text_but_keeps_background() {
        let theme = Theme::blank();
        let cells = [(
            "hidden",
            CellColor::Palette(2),
            CellColor::Palette(1),
            CellAttrs {
                invisible: true,
                bold: true,
                italic: true,
                ..CellAttrs::default()
            },
        )];
        let mut source = FakeFrame::new(1, 1, &cells);
        let geom = FrameGeometry {
            cell_width: 10.0,
            cell_height: 20.0,
            grid_cols: 1,
            grid_rows: 1,
            grid_padding: [0.0; 4],
        };
        let mut bg_cells = Vec::new();
        let mut requests = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut requests);

        assert_eq!(bg_cells[0], [205, 0, 0, 255]);
        assert!(requests.is_empty());
    }

    #[test]
    fn walk_grid_preserves_bold_and_italic_font_attrs() {
        let theme = Theme::blank();
        let cells = [(
            "styled",
            CellColor::Default,
            CellColor::Default,
            CellAttrs {
                bold: true,
                italic: true,
                ..CellAttrs::default()
            },
        )];
        let mut source = FakeFrame::new(1, 1, &cells);
        let geom = FrameGeometry {
            cell_width: 10.0,
            cell_height: 20.0,
            grid_cols: 1,
            grid_rows: 1,
            grid_padding: [0.0; 4],
        };
        let mut bg_cells = Vec::new();
        let mut requests = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut requests);

        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].font_attrs,
            FontAttrs {
                bold: true,
                italic: true,
            }
        );
    }

    #[test]
    fn geometry_honors_user_padding() {
        let cells: [(&str, CellColor, CellColor, CellAttrs); 0] = [];
        let mut source = FakeFrame::new(10, 5, &cells);
        let g = geometry(&mut source, 10.0, 20.0, 200, 200, [12, 6]);
        assert_eq!(g.grid_padding, [12.0, 6.0, 12.0, 6.0]);
    }

    #[test]
    fn geometry_clamps_padding_to_surface() {
        let cells: [(&str, CellColor, CellColor, CellAttrs); 0] = [];
        let mut source = FakeFrame::new(10, 5, &cells);
        let g = geometry(&mut source, 10.0, 20.0, 110, 110, [50, 40]);
        assert_eq!(g.grid_padding[0], 10.0);
        assert_eq!(g.grid_padding[1], 10.0);
    }

    fn ascii_cells_3() -> [(&'static str, CellColor, CellColor, CellAttrs); 3] {
        let plain = CellAttrs::default();
        [
            ("A", CellColor::Default, CellColor::Default, plain),
            ("B", CellColor::Default, CellColor::Default, plain),
            ("C", CellColor::Default, CellColor::Default, plain),
        ]
    }

    #[test]
    fn shape_cache_skips_backend_on_warm_second_frame() {
        let theme = Theme::blank();
        let cells = ascii_cells_3();
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let warmup_calls = backend.shape_calls;
        assert_eq!(warmup_calls, 3, "frame 1 must shape every non-empty cell");
        assert_eq!(builder.shape_cache_stats().misses, 3);
        assert_eq!(builder.shape_cache_stats().hits, 0);

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, warmup_calls,
            "frame 2 must hit the cache for every cell — no new backend calls"
        );
        assert_eq!(builder.shape_cache_stats().hits, 3);
    }

    #[test]
    fn shape_cache_repopulates_after_reset_glyphs() {
        let theme = Theme::blank();
        let cells = ascii_cells_3();
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 3);

        builder.reset_glyphs();
        assert_eq!(builder.shape_cache_stats().hits, 0);
        assert_eq!(builder.shape_cache_stats().misses, 0);

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, 6,
            "after reset_glyphs the cache must be cold again"
        );
    }

    #[test]
    fn shape_cache_unaffected_by_color_changes() {
        // The fg/bg-omission decision means the same character shaped
        // under different theme colors must still hit the cache. This
        // is the test that fails if a future refactor accidentally
        // bakes color into the key.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let warm = [
            ("X", CellColor::Rgb(255, 0, 0), CellColor::Default, plain),
            ("X", CellColor::Rgb(0, 255, 0), CellColor::Default, plain),
            ("X", CellColor::Rgb(0, 0, 255), CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(3, 1, &warm);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        // First "X" is a miss, the next two should hit because the
        // cache key omits color.
        assert_eq!(backend.shape_calls, 1);
        assert_eq!(builder.shape_cache_stats().hits, 2);
        assert_eq!(builder.shape_cache_stats().misses, 1);
    }

    #[test]
    fn shape_cache_distinguishes_bold_and_italic() {
        let theme = Theme::blank();
        let bold = CellAttrs {
            bold: true,
            ..CellAttrs::default()
        };
        let italic = CellAttrs {
            italic: true,
            ..CellAttrs::default()
        };
        let plain = CellAttrs::default();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain),
            ("a", CellColor::Default, CellColor::Default, bold),
            ("a", CellColor::Default, CellColor::Default, italic),
            ("a", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(4, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        // 3 unique (text, attrs) keys: plain, bold, italic. The fourth
        // cell repeats `plain` so it must hit.
        assert_eq!(backend.shape_calls, 3);
        assert_eq!(builder.shape_cache_stats().hits, 1);
        assert_eq!(builder.shape_cache_stats().misses, 3);
    }

    #[test]
    fn shape_cache_warm_hit_rate_above_95_percent() {
        // Synthesize a small but realistic page: 80×24 of repeating
        // ASCII printable characters with a ~10% bold mix. After one
        // warmup frame the working set is bounded by `unique chars × 2
        // styles` ≈ 200 keys; at 1920 cells per frame, hit rate is
        // ~99% on frame 2. Asserting ≥95% gives margin for future
        // changes.
        let theme = Theme::blank();
        let bold = CellAttrs {
            bold: true,
            ..CellAttrs::default()
        };
        let plain = CellAttrs::default();
        let texts: Vec<String> = (0..(80 * 24))
            .map(|i| {
                let c = (b'!' + (i % 94) as u8) as char;
                c.to_string()
            })
            .collect();
        let cells: Vec<(&str, CellColor, CellColor, CellAttrs)> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let attrs = if i % 11 == 0 { bold } else { plain };
                (t.as_str(), CellColor::Default, CellColor::Default, attrs)
            })
            .collect();
        let mut source = FakeFrame::new(80, 24, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        // Warmup frame.
        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let pre = builder.shape_cache_stats();

        // Steady-state frame: identical content.
        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let post = builder.shape_cache_stats();

        let frame2_hits = post.hits - pre.hits;
        let frame2_misses = post.misses - pre.misses;
        let frame2_lookups = frame2_hits + frame2_misses;
        let hit_rate = frame2_hits as f64 / frame2_lookups as f64;
        assert!(
            hit_rate >= 0.95,
            "warm hit rate {hit_rate:.4} < 0.95 (hits={frame2_hits}, lookups={frame2_lookups})"
        );
    }
}
