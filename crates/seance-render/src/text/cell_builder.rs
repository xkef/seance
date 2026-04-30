//! Build per-frame GPU records from a VT snapshot.
//!
//! Two passes:
//! 1. [`walk_grid`] — VT-aware, text-free. Resolves each cell's theme
//!    colors and groups contiguous same-attr cells into a flat
//!    [`CellRun`] list + the bg buffer.
//! 2. [`shape_and_pack`] — text-aware, VT-free. Shapes each run as a
//!    unit through the [`TextBackend`] (so multi-cell ligatures fuse
//!    correctly), ensures each glyph occupies an atlas slot, and emits
//!    one [`CellText`] per glyph at the column of the cell that owns
//!    the glyph's source cluster.

use std::collections::HashMap;

use rustc_hash::FxBuildHasher;
use seance_config::Theme;
use seance_vt::{CellColor, CellView, CellVisitor, DirtySnapshot, FrameSource};

use super::atlas::{AtlasEntry, GlyphAtlas};
use super::backend::{FontAttrs, GlyphFormat, GlyphId, RunGlyph, TextBackend};
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

/// A run of contiguous same-attr cells produced by [`walk_grid`] and
/// consumed by [`shape_and_pack`].
///
/// Cells are column-contiguous in the run (each cell sits one column
/// past the previous). Empty/invisible cells, attribute changes, row
/// boundaries, and wide-char column gaps all break runs. Per-cell fg
/// color is allowed to vary inside one run because color is applied
/// post-shape — it never touches the shaper or the cache key.
struct CellRun {
    row: u16,
    col_start: u16,
    text: String,
    /// Byte offset of each cell's text within `text`. Length = cell
    /// count in the run; the k-th cell is at column `col_start + k`.
    cell_starts: Vec<u16>,
    /// Per-cell fg color, parallel to `cell_starts`.
    cell_fgs: Vec<[u8; 4]>,
    font_attrs: FontAttrs,
}

impl CellRun {
    fn cell_count(&self) -> usize {
        self.cell_starts.len()
    }

    /// Index of the cell whose byte range contains `cluster`, or 0 if
    /// the cluster falls before the first cell (defensive — shouldn't
    /// happen with cosmic-text's clusters).
    fn cell_for_cluster(&self, cluster: u16) -> usize {
        match self.cell_starts.binary_search(&cluster) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        }
    }
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
    shape_cache: ShapeCache,
    bg_cells: Vec<[u8; 4]>,
    text_cells: Vec<CellText>,
    runs: Vec<CellRun>,
    shape_scratch: Vec<RunGlyph>,
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
            runs: Vec::new(),
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
            &mut self.runs,
        );

        self.text_cells.clear();
        shape_and_pack(
            &self.runs,
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
    /// any change that invalidates shape output: font size, scale, or
    /// font family. Family is implicit backend state and not in the
    /// cache key, so swapping families without resetting returns
    /// stale glyph IDs.
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

/// VT-aware, text-free pass: resolve each cell's bg and group
/// contiguous same-attr cells into runs.
fn walk_grid(
    source: &mut dyn FrameSource,
    geom: &FrameGeometry,
    theme: &Theme,
    bg_cells: &mut Vec<[u8; 4]>,
    runs: &mut Vec<CellRun>,
) {
    bg_cells.clear();
    bg_cells.resize(
        geom.grid_cols as usize * geom.grid_rows as usize,
        [0, 0, 0, 0],
    );
    runs.clear();

    let mut visitor = WalkVisitor {
        bg_cells,
        runs,
        theme,
        cols: geom.grid_cols,
        rows: geom.grid_rows,
        current: None,
    };
    source.visit_cells(&mut visitor);
    visitor.flush();
}

/// Text-aware, VT-free pass: shape each run (with cache), cache glyphs,
/// emit one instance record per glyph at the column of the cell whose
/// cluster the glyph belongs to.
fn shape_and_pack(
    runs: &[CellRun],
    backend: &mut dyn TextBackend,
    atlas: &mut GlyphAtlas,
    glyph_slots: &mut GlyphSlots,
    shape_cache: &mut ShapeCache,
    shape_scratch: &mut Vec<RunGlyph>,
    out: &mut Vec<CellText>,
) {
    for run in runs {
        shape_with_cache(
            shape_cache,
            backend,
            &run.text,
            run.font_attrs,
            shape_scratch,
        );
        for rg in &*shape_scratch {
            let Some(entry) = ensure_glyph_slot(glyph_slots, atlas, backend, rg.glyph.id) else {
                continue;
            };
            let cell_idx = run.cell_for_cluster(rg.cluster);
            let col = run.col_start + cell_idx as u16;
            let fg = run.cell_fgs[cell_idx];
            out.push(CellText {
                glyph_pos: entry.pos,
                glyph_size: entry.size,
                bearings: [entry.bearing_x as i16, entry.bearing_y as i16],
                grid_pos: [col, run.row],
                color: fg,
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
    scratch: &mut Vec<RunGlyph>,
) {
    scratch.clear();
    if cache.lookup_into(text, attrs, scratch) {
        return;
    }
    backend.shape_run(text, attrs, scratch);
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

/// Visitor: resolves theme colors, groups visible cells into runs.
/// Holds no backend/atlas references — stays on the VT side of the
/// wall. Caller MUST invoke [`Self::flush`] after the visit completes
/// to push any in-progress run.
struct WalkVisitor<'a> {
    bg_cells: &'a mut Vec<[u8; 4]>,
    runs: &'a mut Vec<CellRun>,
    theme: &'a Theme,
    cols: u16,
    rows: u16,
    current: Option<CellRun>,
}

impl<'a> WalkVisitor<'a> {
    fn flush(&mut self) {
        if let Some(run) = self.current.take() {
            self.runs.push(run);
        }
    }
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
        let fg = [fg_rgb[0], fg_rgb[1], fg_rgb[2], alpha];
        let font_attrs = FontAttrs {
            bold: view.attrs.bold,
            italic: view.attrs.italic,
        };

        // Try to extend the current run, otherwise flush + start a new one.
        // Runs split on row change, font_attrs change, or any non-unit
        // column gap (so wide CJK chars and empty cells both terminate
        // adjacent ASCII runs).
        let extend = self.current.as_ref().is_some_and(|run| {
            run.row == row
                && run.font_attrs == font_attrs
                && col == run.col_start + run.cell_count() as u16
        });

        if extend {
            let run = self
                .current
                .as_mut()
                .expect("current was Some in extend check");
            // Cap run text at u16::MAX bytes so cell_starts can't overflow.
            // 65 535 bytes is far past anything terminal content emits in
            // a single attribute span; the rare overrun starts a new run.
            let next_start = run.text.len();
            if u16::try_from(next_start + view.text.len()).is_err() {
                let stale = self.current.take();
                if let Some(run) = stale {
                    self.runs.push(run);
                }
                self.current = Some(start_run(row, col, view.text, fg, font_attrs));
                return;
            }
            run.cell_starts.push(next_start as u16);
            run.cell_fgs.push(fg);
            run.text.push_str(view.text);
        } else {
            if let Some(prev) = self.current.take() {
                self.runs.push(prev);
            }
            self.current = Some(start_run(row, col, view.text, fg, font_attrs));
        }
    }
}

fn start_run(row: u16, col: u16, text: &str, fg: [u8; 4], font_attrs: FontAttrs) -> CellRun {
    CellRun {
        row,
        col_start: col,
        text: text.to_owned(),
        cell_starts: vec![0],
        cell_fgs: vec![fg],
        font_attrs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::backend::{
        CellMetrics, FontAttrs, GlyphId, RasterizedGlyph, RunGlyph, ShapedGlyph, TextBackend,
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
        fn set_font_features(&mut self, _features: &[String]) {}
        fn shape_run(&mut self, text: &str, _attrs: FontAttrs, out: &mut Vec<RunGlyph>) {
            self.shape_calls += 1;
            // One glyph per char, keyed by codepoint with the byte index
            // as relative cluster — enough for the cell-redistribution
            // tests below to observe whether glyphs land at the right
            // column.
            for (byte_idx, c) in text.char_indices() {
                out.push(RunGlyph {
                    glyph: ShapedGlyph {
                        id: GlyphId(u64::from(u32::from(c))),
                    },
                    cluster: byte_idx as u16,
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
    fn walk_grid_breaks_runs_on_empty_columns() {
        // Empty cell at col 1 must split the row into two runs even
        // though the cells either side share font_attrs.
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(bg_cells.len(), 3);
        assert_eq!(bg_cells[0], [0, 0, 0, 0]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "A");
        assert_eq!(runs[0].col_start, 0);
        assert_eq!(runs[1].text, "B");
        assert_eq!(runs[1].col_start, 2);
        assert_eq!(runs[1].cell_fgs[0], [10, 20, 30, 255]);
        assert_eq!(runs[1].font_attrs, FontAttrs::default());
    }

    #[test]
    fn walk_grid_groups_contiguous_same_attr_cells_into_one_run() {
        // Three plain ASCII cells in a row collapse to one run "ABC".
        // This is the path that lets cosmic-text form ligatures across
        // adjacent cells.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            ("A", CellColor::Default, CellColor::Default, plain),
            ("B", CellColor::Default, CellColor::Default, plain),
            ("C", CellColor::Default, CellColor::Default, plain),
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(runs.len(), 1);
        let run = &runs[0];
        assert_eq!(run.text, "ABC");
        assert_eq!(run.col_start, 0);
        assert_eq!(run.cell_starts, vec![0, 1, 2]);
        assert_eq!(run.cell_fgs.len(), 3);
    }

    #[test]
    fn walk_grid_splits_on_font_attr_change() {
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let bold = CellAttrs {
            bold: true,
            ..CellAttrs::default()
        };
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain),
            ("a", CellColor::Default, CellColor::Default, bold),
            ("a", CellColor::Default, CellColor::Default, plain),
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(runs.len(), 3);
        assert!(!runs[0].font_attrs.bold);
        assert!(runs[1].font_attrs.bold);
        assert!(!runs[2].font_attrs.bold);
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cell_fgs[0], [0, 205, 205, FAINT_ALPHA]);
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(bg_cells[0], [205, 0, 0, 255]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].cell_fgs[0], [0, 0, 0, 255]);
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(bg_cells[0], [205, 0, 0, 255]);
        assert!(runs.is_empty());
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
        let mut runs = Vec::new();

        walk_grid(&mut source, &geom, &theme, &mut bg_cells, &mut runs);

        assert_eq!(runs.len(), 1);
        assert_eq!(
            runs[0].font_attrs,
            FontAttrs {
                bold: true,
                italic: true,
            }
        );
    }

    #[test]
    fn cell_for_cluster_maps_glyph_bytes_back_to_cell_index() {
        let run = CellRun {
            row: 0,
            col_start: 5,
            text: "->=>".to_owned(),
            cell_starts: vec![0, 1, 2, 3],
            cell_fgs: vec![[0; 4]; 4],
            font_attrs: FontAttrs::default(),
        };
        // A cluster at byte 0 maps to the first cell, byte 2 to the
        // third, and a stray cluster past the end pins to the last.
        assert_eq!(run.cell_for_cluster(0), 0);
        assert_eq!(run.cell_for_cluster(1), 1);
        assert_eq!(run.cell_for_cluster(2), 2);
        assert_eq!(run.cell_for_cluster(3), 3);
        assert_eq!(run.cell_for_cluster(99), 3);
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
        // Three contiguous plain cells collapse to one run "ABC" — one
        // backend call per frame. Frame 2 must hit the cache instead of
        // re-shaping.
        let theme = Theme::blank();
        let cells = ascii_cells_3();
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let warmup_calls = backend.shape_calls;
        assert_eq!(warmup_calls, 1, "frame 1 must shape one run");
        assert_eq!(builder.shape_cache_stats().misses, 1);
        assert_eq!(builder.shape_cache_stats().hits, 0);

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, warmup_calls,
            "frame 2 must hit the cache — no new backend calls"
        );
        assert_eq!(builder.shape_cache_stats().hits, 1);
    }

    #[test]
    fn shape_cache_repopulates_after_reset_glyphs() {
        let theme = Theme::blank();
        let cells = ascii_cells_3();
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 1);

        builder.reset_glyphs();
        assert_eq!(builder.shape_cache_stats().hits, 0);
        assert_eq!(builder.shape_cache_stats().misses, 0);

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, 2,
            "after reset_glyphs the cache must be cold again"
        );
    }

    #[test]
    fn shape_cache_unaffected_by_color_changes() {
        // Identical run text on different rows under different colors
        // must still hit the cache — the key omits color. Guards
        // against a refactor that bakes color into the key. Three rows
        // (rather than three columns) keep them as separate runs of
        // identical text.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let warm = [
            ("X", CellColor::Rgb(255, 0, 0), CellColor::Default, plain),
            ("X", CellColor::Rgb(0, 255, 0), CellColor::Default, plain),
            ("X", CellColor::Rgb(0, 0, 255), CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(1, 3, &warm);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        // First "X" is a miss, the next two rows should hit because
        // the cache key omits color.
        assert_eq!(backend.shape_calls, 1);
        assert_eq!(builder.shape_cache_stats().hits, 2);
        assert_eq!(builder.shape_cache_stats().misses, 1);
    }

    #[test]
    fn shape_cache_reuses_run_across_grid_positions() {
        // Issue #205's headline acceptance: the same run text appearing
        // at different columns / rows must shape exactly once.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            // Row 0: "foo" at col 0, gap, "foo" at col 4.
            ("f", CellColor::Default, CellColor::Default, plain),
            ("o", CellColor::Default, CellColor::Default, plain),
            ("o", CellColor::Default, CellColor::Default, plain),
            ("", CellColor::Default, CellColor::Default, plain),
            ("f", CellColor::Default, CellColor::Default, plain),
            ("o", CellColor::Default, CellColor::Default, plain),
            ("o", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(7, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, 1,
            "two runs of identical text must share one shape result"
        );
        assert_eq!(builder.shape_cache_stats().hits, 1);
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
        // 80×24 of repeating ASCII printable chars with ~10% bold.
        // Working set ≈ 94 chars × 2 styles ≈ 200 keys; warm hit rate
        // is near 100%. The 95% threshold leaves margin for any
        // future change to the key shape.
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
