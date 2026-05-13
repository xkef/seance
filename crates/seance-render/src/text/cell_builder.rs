//! Build per-frame GPU records from a VT snapshot.
//!
//! Two passes:
//! 1. [`walk_grid_into_runs`] — VT-aware, text-free. Resolves each cell's
//!    theme colors, writes the bg buffer, and groups contiguous same-style
//!    cells into [`ShapeRun`]s as the visitor walks.
//! 2. [`shape_runs`] — text-aware, VT-free. Shapes each run through the
//!    [`TextBackend`], ensures every glyph occupies an atlas slot, and emits
//!    one [`CellText`] per glyph anchored at the column of its source cell.
//!
//! [`ShapeRun`] is the natural unit of shaping: harfbuzz can only compose
//! ligatures, ZWJ sequences, regional flag pairs, and combining marks when
//! it sees the whole cluster. Building runs in the visitor — rather than
//! reconstructing them later from a flat per-cell list — keeps the pipeline
//! direct and lets each run own its cluster→column map locally.

use std::collections::HashMap;

use rustc_hash::FxBuildHasher;
use seance_config::Theme;
use seance_frame::{CellView, CellVisitor, FrameSource};
use seance_protocol::frame::{CellColor, DirtySnapshot};

use super::atlas::{AtlasEntry, GlyphAtlas};
use super::backend::{CellMetrics, FontAttrs, GlyphFormat, GlyphId, ShapedGlyph, TextBackend};
use super::procedural;
use super::shape_cache::ShapeCache;

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

const TEXT_FLAG_MIN_CONTRAST: u32 = 1 << 16;

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
type ProceduralSlots = HashMap<char, AtlasEntry, FxBuildHasher>;

/// Foreground alpha applied to faint cells (SGR 2). Other attributes are
/// represented at full opacity.
const FAINT_ALPHA: u8 = 128;

struct FrameGeometry {
    cell_width: f32,
    cell_height: f32,
    grid_cols: u16,
    grid_rows: u16,
    grid_padding: [f32; 4],
}

/// One cell within a [`ShapeRun`]. `byte_offset` is the start of this cell's
/// grapheme inside the run's `text`; `col` is the grid column the cell lives
/// at; `fg` is the post-inverse, post-faint foreground color.
#[derive(Debug, Clone, Copy)]
struct CellSlot {
    byte_offset: u32,
    col: u16,
    fg: [u8; 4],
    min_contrast: bool,
}

/// A single grid cell that bypassed shaping because its grapheme is a
/// procedurally-rasterized codepoint (box-drawing, block elements). Buffered
/// during the visitor pass and emitted as `CellText` records alongside the
/// shape-run output.
#[derive(Debug, Clone, Copy)]
struct ProceduralCell {
    row: u16,
    col: u16,
    codepoint: char,
    fg: [u8; 4],
    min_contrast: bool,
}

/// A contiguous group of cells on the same row sharing one [`FontAttrs`],
/// shaped as a single unit so harfbuzz can compose across cell boundaries.
///
/// The run owns its concatenated `text` plus a `slots` array sorted by
/// `byte_offset`. After shaping, each emitted glyph reports its source
/// cluster (a byte offset within `text`); [`Self::slot_for_cluster`]
/// binary-searches `slots` to find the originating cell, and the glyph is
/// anchored at that cell's column. A multi-cell ligature with `cluster = 0`
/// lands on the first slot's column; the trailing cells emit no `CellText`
/// because the GPU buffer is glyph-indexed (see `gpu/state.rs`).
struct ShapeRun {
    row: u16,
    font_attrs: FontAttrs,
    text: String,
    slots: Vec<CellSlot>,
}

impl ShapeRun {
    fn new(row: u16, font_attrs: FontAttrs) -> Self {
        Self {
            row,
            font_attrs,
            text: String::new(),
            slots: Vec::new(),
        }
    }

    fn push_cell(&mut self, col: u16, fg: [u8; 4], min_contrast: bool, text: &str) {
        self.slots.push(CellSlot {
            byte_offset: self.text.len() as u32,
            col,
            fg,
            min_contrast,
        });
        self.text.push_str(text);
    }

    /// Whether `(row, col, font_attrs)` continues this run: same row, same
    /// attrs, and column directly following the last slot's column.
    fn extends(&self, row: u16, col: u16, font_attrs: FontAttrs) -> bool {
        self.row == row
            && self.font_attrs == font_attrs
            && self.slots.last().is_some_and(|s| s.col + 1 == col)
    }

    /// Locate the slot whose grapheme covers byte offset `cluster`. Matches
    /// Ghostty's anchor-at-cluster-start rule: a ligature with `cluster = 0`
    /// resolves to the first slot, and a sub-grapheme offset (e.g. byte 1 of
    /// a 2-byte "é") resolves to the slot that opened it.
    fn slot_for_cluster(&self, cluster: u32) -> &CellSlot {
        debug_assert!(!self.slots.is_empty(), "shape run flushed without cells");
        let idx = match self.slots.binary_search_by_key(&cluster, |s| s.byte_offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        &self.slots[idx]
    }
}

pub struct CellBuilder {
    atlas: GlyphAtlas,
    /// Stable map from backend-issued `GlyphId` to its atlas slot. Survives
    /// across frames; cleared by [`Self::reset_glyphs`].
    glyph_slots: GlyphSlots,
    /// Atlas slot per procedural codepoint (box-drawing / block elements).
    /// Keyed by `char` because procedural rasterization is metric-stable —
    /// the same codepoint rasterizes the same bitmap at every call for a
    /// given `CellMetrics`. Cleared whenever the cell size changes, via
    /// [`Self::reset_glyphs`].
    procedural_slots: ProceduralSlots,
    shape_cache: ShapeCache,
    bg_cells: Vec<[u8; 4]>,
    text_cells: Vec<CellText>,
    runs: Vec<ShapeRun>,
    procedural_cells: Vec<ProceduralCell>,
    shape_scratch: Vec<ShapedGlyph>,
    last_frame: Option<FrameInfo>,
    last_dirty: DirtySnapshot,
}

impl CellBuilder {
    pub fn new() -> Self {
        Self {
            atlas: GlyphAtlas::new(),
            glyph_slots: HashMap::with_hasher(FxBuildHasher),
            procedural_slots: HashMap::with_hasher(FxBuildHasher),
            shape_cache: ShapeCache::new(),
            bg_cells: Vec::new(),
            text_cells: Vec::new(),
            runs: Vec::new(),
            procedural_cells: Vec::new(),
            shape_scratch: Vec::new(),
            last_frame: None,
            // First frame must be a full upload — there's nothing on the GPU
            // yet for a `Partial` write to layer onto.
            last_dirty: DirtySnapshot::Full,
        }
    }

    pub fn build_frame(
        &mut self,
        source: &mut dyn FrameSource,
        backend: &mut dyn TextBackend,
        config: BuildFrameConfig<'_>,
    ) {
        // Sample the dirty set first so the rest of the build can mutate the
        // source freely. Immutable VT snapshots ignore `clear_dirty`; the app
        // acknowledges rendered generations through the VT actor after present.
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

        walk_grid_into_runs(
            source,
            &geom,
            config.theme,
            &mut self.bg_cells,
            &mut self.runs,
            &mut self.procedural_cells,
        );

        self.text_cells.clear();
        shape_runs(
            &self.runs,
            backend,
            &mut self.atlas,
            &mut self.glyph_slots,
            &mut self.shape_cache,
            &mut self.shape_scratch,
            &mut self.text_cells,
        );
        emit_procedural_cells(
            &self.procedural_cells,
            backend.metrics(),
            &mut self.atlas,
            &mut self.procedural_slots,
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

    /// Drop all atlas-cached glyphs and shape cache entries. Call on any
    /// change that invalidates shape output: font size, scale, family, or
    /// active features. Family is implicit backend state and not in the
    /// cache key, so swapping families without resetting returns stale glyph
    /// IDs.
    ///
    /// Procedural slots are also dropped because their bitmaps are sized to
    /// the current `CellMetrics`; a font-size change resizes the cell.
    pub fn reset_glyphs(&mut self) {
        self.atlas.reset();
        self.glyph_slots.clear();
        self.procedural_slots.clear();
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

/// Resolve a VT-reported color into concrete RGB. `None` means "use the
/// theme default" — the caller decides fg vs bg.
fn resolve_color(theme: &Theme, color: &CellColor) -> Option<[u8; 3]> {
    match *color {
        CellColor::Default => None,
        CellColor::Palette(idx) => Some(theme.palette[idx as usize]),
        CellColor::Rgb(r, g, b) => Some([r, g, b]),
    }
}

/// VT-aware pass: walk every cell, write its bg into `bg_cells`, and
/// accumulate non-empty cells into [`ShapeRun`]s grouped by row, attrs, and
/// column contiguity. Cells whose grapheme is a single procedural codepoint
/// (box-drawing, block elements) bypass shaping and land in `procedural_cells`
/// instead.
fn walk_grid_into_runs(
    source: &mut dyn FrameSource,
    geom: &FrameGeometry,
    theme: &Theme,
    bg_cells: &mut Vec<[u8; 4]>,
    runs: &mut Vec<ShapeRun>,
    procedural_cells: &mut Vec<ProceduralCell>,
) {
    bg_cells.clear();
    bg_cells.resize(
        geom.grid_cols as usize * geom.grid_rows as usize,
        [0, 0, 0, 0],
    );
    runs.clear();
    procedural_cells.clear();

    let mut visitor = RunBuilder {
        bg_cells,
        runs,
        procedural_cells,
        open: None,
        theme,
        cols: geom.grid_cols,
        rows: geom.grid_rows,
    };
    source.visit_cells(&mut visitor);
    visitor.flush();
}

/// Text-aware pass: shape each run through the cache, ensure every glyph
/// has an atlas slot, and emit a [`CellText`] anchored at the originating
/// cell's column. Glyphs whose rasterization yields nothing (whitespace,
/// zero-sized) are silently skipped.
fn shape_runs(
    runs: &[ShapeRun],
    backend: &mut dyn TextBackend,
    atlas: &mut GlyphAtlas,
    glyph_slots: &mut GlyphSlots,
    shape_cache: &mut ShapeCache,
    shape_scratch: &mut Vec<ShapedGlyph>,
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
        for glyph in &*shape_scratch {
            let Some(entry) = ensure_glyph_slot(glyph_slots, atlas, backend, glyph.id) else {
                continue;
            };
            let slot = run.slot_for_cluster(glyph.cluster);
            let mut atlas_and_flags = u32::from(entry.is_color);
            if slot.min_contrast {
                atlas_and_flags |= TEXT_FLAG_MIN_CONTRAST;
            }
            out.push(CellText {
                glyph_pos: entry.pos,
                glyph_size: entry.size,
                bearings: [entry.bearing_x as i16, entry.bearing_y as i16],
                grid_pos: [slot.col, run.row],
                color: slot.fg,
                atlas_and_flags,
            });
        }
    }
}

/// Run `text` through the shape cache, falling through to `backend` on
/// miss and inserting the result. Always leaves `scratch` holding the
/// shaped glyphs for the caller.
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

/// `CellVisitor` that resolves theme colors, writes the bg buffer, and
/// builds [`ShapeRun`]s on the fly. Maintains a single `open` run that
/// extends while incoming cells share its row, attrs, and column
/// contiguity; any break flushes `open` into `runs` and starts fresh.
struct RunBuilder<'a> {
    bg_cells: &'a mut Vec<[u8; 4]>,
    runs: &'a mut Vec<ShapeRun>,
    procedural_cells: &'a mut Vec<ProceduralCell>,
    open: Option<ShapeRun>,
    theme: &'a Theme,
    cols: u16,
    rows: u16,
}

impl RunBuilder<'_> {
    /// Move any open run into `runs`. Idempotent — safe to call from
    /// per-cell flush points and once more at end-of-walk.
    fn flush(&mut self) {
        if let Some(run) = self.open.take() {
            self.runs.push(run);
        }
    }
}

impl CellVisitor for RunBuilder<'_> {
    fn cell(&mut self, row: u16, col: u16, view: CellView<'_>) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let idx = row as usize * self.cols as usize + col as usize;

        let theme_bg = [self.theme.bg[0], self.theme.bg[1], self.theme.bg[2]];
        let min_contrast = matches!(view.fg, CellColor::Default) && !view.attrs.inverse;
        let mut fg_rgb = resolve_color(self.theme, &view.fg).unwrap_or(self.theme.fg);
        let mut bg_rgb = resolve_color(self.theme, &view.bg).unwrap_or(theme_bg);
        if view.attrs.inverse {
            std::mem::swap(&mut fg_rgb, &mut bg_rgb);
        }
        if bg_rgb != theme_bg {
            self.bg_cells[idx] = [bg_rgb[0], bg_rgb[1], bg_rgb[2], 255];
        }

        // Empty / invisible cells emit no glyph and break any open run.
        if view.text.is_empty() || view.attrs.invisible {
            self.flush();
            return;
        }

        let alpha = if view.attrs.faint { FAINT_ALPHA } else { 255 };
        let fg = [fg_rgb[0], fg_rgb[1], fg_rgb[2], alpha];

        // Procedural codepoints (box-drawing, block elements) bypass shaping
        // and rasterize directly into the grayscale atlas. They always break
        // any open shape run because they don't participate in cluster
        // composition with neighbouring cells.
        if let Some(c) = procedural_codepoint(view.text) {
            self.flush();
            self.procedural_cells.push(ProceduralCell {
                row,
                col,
                codepoint: c,
                fg,
                min_contrast,
            });
            return;
        }

        let attrs = FontAttrs {
            bold: view.attrs.bold,
            italic: view.attrs.italic,
        };

        let extends = self
            .open
            .as_ref()
            .is_some_and(|r| r.extends(row, col, attrs));
        if !extends {
            self.flush();
            self.open = Some(ShapeRun::new(row, attrs));
        }
        // Either the existing run extends, or the branch above just opened
        // a fresh one — `open` is `Some` either way.
        self.open
            .as_mut()
            .expect("open run set above")
            .push_cell(col, fg, min_contrast, view.text);
    }
}

/// If `text` is a single-codepoint grapheme that has a procedural renderer,
/// return that codepoint; otherwise `None`. Bold/italic attrs are ignored
/// because box-drawing characters render identically under any style.
fn procedural_codepoint(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    if procedural::supports(c) {
        Some(c)
    } else {
        None
    }
}

/// Rasterize each unique procedural codepoint once into the grayscale atlas,
/// then emit one [`CellText`] per buffered procedural cell anchored at its
/// grid position.
fn emit_procedural_cells(
    cells: &[ProceduralCell],
    metrics: &CellMetrics,
    atlas: &mut GlyphAtlas,
    slots: &mut ProceduralSlots,
    out: &mut Vec<CellText>,
) {
    for cell in cells {
        let entry = match slots.get(&cell.codepoint) {
            Some(e) => *e,
            None => {
                let Some(rast) = procedural::rasterize(cell.codepoint, metrics) else {
                    continue;
                };
                let Some(entry) = atlas.insert(
                    &rast.data,
                    rast.width,
                    rast.height,
                    rast.bearing_x,
                    rast.bearing_y,
                    matches!(rast.format, GlyphFormat::Color),
                ) else {
                    continue;
                };
                slots.insert(cell.codepoint, entry);
                entry
            }
        };
        let mut atlas_and_flags = u32::from(entry.is_color);
        if cell.min_contrast {
            atlas_and_flags |= TEXT_FLAG_MIN_CONTRAST;
        }
        out.push(CellText {
            glyph_pos: entry.pos,
            glyph_size: entry.size,
            bearings: [entry.bearing_x as i16, entry.bearing_y as i16],
            grid_pos: [cell.col, cell.row],
            color: cell.fg,
            atlas_and_flags,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::backend::{
        CellMetrics, FontAttrs, GlyphId, RasterizedGlyph, ShapedGlyph, TextBackend,
    };
    use seance_frame::CellVisitor;
    use seance_protocol::frame::{CellAttrs, CellColor, CursorInfo, GridPos};

    type FakeCell<'a> = (&'a str, CellColor, CellColor, CellAttrs);

    struct FakeFrame<'a> {
        cols: u16,
        rows: u16,
        cells: &'a [FakeCell<'a>],
        dirty: DirtySnapshot,
        clear_count: u32,
    }

    impl<'a> FakeFrame<'a> {
        fn new(cols: u16, rows: u16, cells: &'a [FakeCell<'a>]) -> Self {
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
                        hyperlink: None,
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

    /// Stub backend: emits one glyph per char at the char's byte offset.
    /// Lets tests observe both shape-cache behavior and cluster→column
    /// mapping by the glyph stream alone.
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
        fn set_adjust_cell_width(&mut self, _value: Option<&str>) {}
        fn set_features(&mut self, _features: &[String]) {}
        fn set_fallback(&mut self, _fallback: &[String]) {}
        fn shape_run(&mut self, text: &str, _attrs: FontAttrs, out: &mut Vec<ShapedGlyph>) {
            self.shape_calls += 1;
            let mut byte_offset = 0u32;
            for c in text.chars() {
                out.push(ShapedGlyph {
                    id: GlyphId(u64::from(u32::from(c))),
                    cluster: byte_offset,
                });
                byte_offset += c.len_utf8() as u32;
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

    /// Run only the visitor pass and return the bg buffer + accumulated
    /// runs. Collapses the boilerplate of constructing a FrameGeometry +
    /// FakeFrame for each visitor-level test.
    fn collect_runs(
        cols: u16,
        rows: u16,
        cells: &[FakeCell<'_>],
        theme: &Theme,
    ) -> (Vec<[u8; 4]>, Vec<ShapeRun>) {
        let (bg, runs, _) = collect_runs_with_procedural(cols, rows, cells, theme);
        (bg, runs)
    }

    /// Variant that also returns the procedural-cell buffer so the
    /// box-drawing interception tests can observe it.
    fn collect_runs_with_procedural(
        cols: u16,
        rows: u16,
        cells: &[FakeCell<'_>],
        theme: &Theme,
    ) -> (Vec<[u8; 4]>, Vec<ShapeRun>, Vec<ProceduralCell>) {
        let mut source = FakeFrame::new(cols, rows, cells);
        let geom = FrameGeometry {
            cell_width: 10.0,
            cell_height: 20.0,
            grid_cols: cols,
            grid_rows: rows,
            grid_padding: [0.0; 4],
        };
        let mut bg = Vec::new();
        let mut runs = Vec::new();
        let mut procedural = Vec::new();
        walk_grid_into_runs(
            &mut source,
            &geom,
            theme,
            &mut bg,
            &mut runs,
            &mut procedural,
        );
        (bg, runs, procedural)
    }

    fn plain() -> CellAttrs {
        CellAttrs::default()
    }

    fn bold() -> CellAttrs {
        CellAttrs {
            bold: true,
            ..CellAttrs::default()
        }
    }

    fn italic() -> CellAttrs {
        CellAttrs {
            italic: true,
            ..CellAttrs::default()
        }
    }

    // ────────────────────────────────────────────────────────────────────
    // build_frame end-to-end
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn build_frame_captures_dirty_snapshot_and_acknowledges_source() {
        let theme = Theme::blank();
        let cells = [("A", CellColor::Default, CellColor::Default, plain())];
        let mut source = FakeFrame::new(1, 1, &cells);
        source.dirty = DirtySnapshot::Partial(vec![0]);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));

        assert_eq!(*builder.last_dirty(), DirtySnapshot::Partial(vec![0]));
        assert_eq!(source.clear_count, 1);
        // Sticky-clear simulation: the second build sees Clean because the
        // fake reset itself in clear_dirty.
        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(*builder.last_dirty(), DirtySnapshot::Clean);
        assert_eq!(source.clear_count, 2);
    }

    #[test]
    fn build_frame_first_call_defaults_last_dirty_to_full_until_sampled() {
        // Before any build_frame, last_dirty is Full so the first GPU
        // upload is the full path.
        let builder = CellBuilder::new();
        assert_eq!(*builder.last_dirty(), DirtySnapshot::Full);
    }

    #[test]
    fn geometry_honors_user_padding() {
        let cells: [FakeCell<'_>; 0] = [];
        let mut source = FakeFrame::new(10, 5, &cells);
        let g = geometry(&mut source, 10.0, 20.0, 200, 200, [12, 6]);
        assert_eq!(g.grid_padding, [12.0, 6.0, 12.0, 6.0]);
    }

    #[test]
    fn geometry_clamps_padding_to_surface() {
        let cells: [FakeCell<'_>; 0] = [];
        let mut source = FakeFrame::new(10, 5, &cells);
        let g = geometry(&mut source, 10.0, 20.0, 110, 110, [50, 40]);
        assert_eq!(g.grid_padding[0], 10.0);
        assert_eq!(g.grid_padding[1], 10.0);
    }

    // ────────────────────────────────────────────────────────────────────
    // Visitor pass: walk_grid_into_runs / RunBuilder
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn walk_grid_into_runs_emits_one_slot_per_non_empty_cell() {
        let theme = Theme::blank();
        let cells = [
            ("A", CellColor::Default, CellColor::Default, plain()),
            ("", CellColor::Default, CellColor::Default, plain()),
            ("B", CellColor::Rgb(10, 20, 30), CellColor::Default, plain()),
        ];
        let (bg, runs) = collect_runs(3, 1, &cells, &theme);

        assert_eq!(bg.len(), 3);
        assert_eq!(bg[0], [0, 0, 0, 0]);
        // Empty middle cell breaks contiguity → two single-slot runs.
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "A");
        assert_eq!(runs[0].slots[0].col, 0);
        assert_eq!(runs[1].text, "B");
        assert_eq!(runs[1].slots[0].col, 2);
        assert_eq!(runs[1].slots[0].fg, [10, 20, 30, 255]);
        assert_eq!(runs[1].font_attrs, FontAttrs::default());
    }

    #[test]
    fn walk_grid_into_runs_applies_faint_alpha() {
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
        let (_bg, runs) = collect_runs(1, 1, &cells, &theme);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].slots[0].fg, [0, 205, 205, FAINT_ALPHA]);
    }

    #[test]
    fn walk_grid_into_runs_marks_only_default_fg_for_min_contrast() {
        let theme = Theme::blank();
        let cells = [
            ("d", CellColor::Default, CellColor::Default, plain()),
            ("p", CellColor::Palette(1), CellColor::Default, plain()),
            ("r", CellColor::Rgb(10, 20, 30), CellColor::Default, plain()),
            (
                "i",
                CellColor::Default,
                CellColor::Palette(2),
                CellAttrs {
                    inverse: true,
                    ..CellAttrs::default()
                },
            ),
        ];
        let (_bg, runs) = collect_runs(4, 1, &cells, &theme);

        assert_eq!(runs.len(), 1);
        let flags: Vec<bool> = runs[0].slots.iter().map(|s| s.min_contrast).collect();
        assert_eq!(flags, vec![true, false, false, false]);
    }

    #[test]
    fn walk_grid_into_runs_swaps_fg_and_bg_for_inverse_cells() {
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
        let (bg, runs) = collect_runs(1, 1, &cells, &theme);

        assert_eq!(bg[0], [205, 0, 0, 255]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].slots[0].fg, [0, 0, 0, 255]);
    }

    #[test]
    fn walk_grid_into_runs_skips_invisible_text_but_keeps_background() {
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
        let (bg, runs) = collect_runs(1, 1, &cells, &theme);

        assert_eq!(bg[0], [205, 0, 0, 255]);
        assert!(runs.is_empty());
    }

    #[test]
    fn walk_grid_into_runs_preserves_bold_and_italic_font_attrs() {
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
        let (_bg, runs) = collect_runs(1, 1, &cells, &theme);

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
    fn run_builder_emits_two_slots_for_contiguous_cells() {
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            ("b", CellColor::Default, CellColor::Default, plain()),
        ];
        let (_bg, runs) = collect_runs(2, 1, &cells, &theme);

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "ab");
        let offsets: Vec<u32> = runs[0].slots.iter().map(|s| s.byte_offset).collect();
        assert_eq!(offsets, vec![0, 1]);
        let cols: Vec<u16> = runs[0].slots.iter().map(|s| s.col).collect();
        assert_eq!(cols, vec![0, 1]);
    }

    #[test]
    fn run_builder_flushes_on_invisible_then_reopens() {
        // Invisible cells flush an open run without emitting a slot. The
        // following visible cell starts a fresh run, even though attrs and
        // row are identical.
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            (
                "X",
                CellColor::Default,
                CellColor::Default,
                CellAttrs {
                    invisible: true,
                    ..CellAttrs::default()
                },
            ),
            ("b", CellColor::Default, CellColor::Default, plain()),
        ];
        let (_bg, runs) = collect_runs(3, 1, &cells, &theme);

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].text, "a");
        assert_eq!(runs[1].text, "b");
    }

    #[test]
    fn run_builder_does_not_emit_empty_run_at_eof() {
        let theme = Theme::blank();
        let cells = [
            ("", CellColor::Default, CellColor::Default, plain()),
            ("", CellColor::Default, CellColor::Default, plain()),
        ];
        let (bg, runs) = collect_runs(2, 1, &cells, &theme);

        assert!(runs.is_empty());
        assert_eq!(bg.len(), 2);
    }

    // ────────────────────────────────────────────────────────────────────
    // ShapeRun primitives
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn shape_run_slot_for_cluster_handles_mid_grapheme() {
        // Three cells "a", "é" (2 bytes), "b" → text "aéb",
        // slots at byte_offset [0, 1, 3].
        let mut run = ShapeRun::new(0, FontAttrs::default());
        run.push_cell(0, [1, 1, 1, 255], true, "a");
        run.push_cell(1, [2, 2, 2, 255], true, "é");
        run.push_cell(2, [3, 3, 3, 255], true, "b");

        assert_eq!(run.slot_for_cluster(0).col, 0);
        assert_eq!(run.slot_for_cluster(1).col, 1);
        // Byte 2 falls inside é — anchor at the slot that opened it.
        assert_eq!(run.slot_for_cluster(2).col, 1);
        assert_eq!(run.slot_for_cluster(3).col, 2);
    }

    // ────────────────────────────────────────────────────────────────────
    // Run-grouping behavior observed via build_frame
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn run_grouping_collapses_contiguous_same_style_cells() {
        let theme = Theme::blank();
        let cells = [
            ("h", CellColor::Default, CellColor::Default, plain()),
            ("e", CellColor::Default, CellColor::Default, plain()),
            ("l", CellColor::Default, CellColor::Default, plain()),
            ("l", CellColor::Default, CellColor::Default, plain()),
            ("o", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(5, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, 1,
            "5 contiguous plain cells must collapse into one shape_run call"
        );
    }

    #[test]
    fn run_grouping_breaks_on_attr_change() {
        // [plain plain bold plain] → 3 runs.
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            ("b", CellColor::Default, CellColor::Default, plain()),
            ("c", CellColor::Default, CellColor::Default, bold()),
            ("d", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(4, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 3);
    }

    #[test]
    fn run_grouping_breaks_on_empty_cell_gap() {
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            ("b", CellColor::Default, CellColor::Default, plain()),
            ("", CellColor::Default, CellColor::Default, plain()),
            ("c", CellColor::Default, CellColor::Default, plain()),
            ("d", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(5, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 2);
    }

    #[test]
    fn run_grouping_breaks_across_rows() {
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            ("b", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(1, 2, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 2);
    }

    // ────────────────────────────────────────────────────────────────────
    // Glyph distribution: cluster → column anchoring
    // ────────────────────────────────────────────────────────────────────

    /// Backend that emits one rasterized 1×1 glyph per char at the char's
    /// byte offset. Lets tests observe the column each glyph lands at.
    struct DistributingBackend {
        metrics: CellMetrics,
    }

    impl TextBackend for DistributingBackend {
        fn metrics(&self) -> &CellMetrics {
            &self.metrics
        }
        fn set_font_size(&mut self, _: f32) {}
        fn set_scale(&mut self, _: f64) {}
        fn set_adjust_cell_height(&mut self, _: Option<&str>) {}
        fn set_adjust_cell_width(&mut self, _: Option<&str>) {}
        fn set_features(&mut self, _: &[String]) {}
        fn set_fallback(&mut self, _: &[String]) {}
        fn shape_run(&mut self, text: &str, _: FontAttrs, out: &mut Vec<ShapedGlyph>) {
            let mut byte = 0u32;
            for c in text.chars() {
                out.push(ShapedGlyph {
                    id: GlyphId(u64::from(u32::from(c))),
                    cluster: byte,
                });
                byte += c.len_utf8() as u32;
            }
        }
        fn rasterize(&mut self, _: GlyphId) -> Option<RasterizedGlyph> {
            Some(RasterizedGlyph {
                data: vec![255],
                width: 1,
                height: 1,
                bearing_x: 0,
                bearing_y: 0,
                format: GlyphFormat::Alpha,
            })
        }
    }

    fn distributing_backend() -> DistributingBackend {
        DistributingBackend {
            metrics: CellMetrics {
                cell_width: 10.0,
                cell_height: 20.0,
                baseline: 16.0,
            },
        }
    }

    #[test]
    fn cluster_to_column_distributes_glyphs_back_to_source_cells() {
        let theme = Theme::blank();
        let cells = [
            ("X", CellColor::Default, CellColor::Default, plain()),
            ("Y", CellColor::Default, CellColor::Default, plain()),
            ("Z", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = distributing_backend();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let cols: Vec<u16> = builder.text_cells().iter().map(|c| c.grid_pos[0]).collect();
        assert_eq!(cols, vec![0, 1, 2]);
    }

    #[test]
    fn min_contrast_flag_only_set_for_default_foreground_glyphs() {
        let theme = Theme::blank();
        let cells = [
            ("D", CellColor::Default, CellColor::Default, plain()),
            ("P", CellColor::Palette(1), CellColor::Default, plain()),
            ("R", CellColor::Rgb(10, 20, 30), CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = distributing_backend();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let flags: Vec<bool> = builder
            .text_cells()
            .iter()
            .map(|c| c.atlas_and_flags & TEXT_FLAG_MIN_CONTRAST != 0)
            .collect();
        assert_eq!(flags, vec![true, false, false]);
    }

    /// Backend that returns a single ligature glyph at cluster 0 for "==".
    struct LigatureBackend {
        metrics: CellMetrics,
    }

    impl TextBackend for LigatureBackend {
        fn metrics(&self) -> &CellMetrics {
            &self.metrics
        }
        fn set_font_size(&mut self, _: f32) {}
        fn set_scale(&mut self, _: f64) {}
        fn set_adjust_cell_height(&mut self, _: Option<&str>) {}
        fn set_adjust_cell_width(&mut self, _: Option<&str>) {}
        fn set_features(&mut self, _: &[String]) {}
        fn set_fallback(&mut self, _: &[String]) {}
        fn shape_run(&mut self, text: &str, _: FontAttrs, out: &mut Vec<ShapedGlyph>) {
            if text == "==" {
                out.push(ShapedGlyph {
                    id: GlyphId(0xEE),
                    cluster: 0,
                });
            }
        }
        fn rasterize(&mut self, _: GlyphId) -> Option<RasterizedGlyph> {
            Some(RasterizedGlyph {
                data: vec![255],
                width: 1,
                height: 1,
                bearing_x: 0,
                bearing_y: 0,
                format: GlyphFormat::Alpha,
            })
        }
    }

    #[test]
    fn ligature_glyph_anchors_at_first_cell_only() {
        // Two-cell run "==" → one CellText at column 0; trailing column 1
        // is left blank because the GPU buffer is glyph-indexed.
        let theme = Theme::blank();
        let cells = [
            ("=", CellColor::Default, CellColor::Default, plain()),
            ("=", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(2, 1, &cells);
        let mut backend = LigatureBackend {
            metrics: CellMetrics {
                cell_width: 10.0,
                cell_height: 20.0,
                baseline: 16.0,
            },
        };
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(builder.text_cells().len(), 1);
        assert_eq!(builder.text_cells()[0].grid_pos, [0, 0]);
    }

    // ────────────────────────────────────────────────────────────────────
    // Shape cache behavior (observed through build_frame)
    // ────────────────────────────────────────────────────────────────────

    fn ascii_cells_3() -> [FakeCell<'static>; 3] {
        [
            ("A", CellColor::Default, CellColor::Default, plain()),
            ("B", CellColor::Default, CellColor::Default, plain()),
            ("C", CellColor::Default, CellColor::Default, plain()),
        ]
    }

    #[test]
    fn shape_cache_skips_backend_on_warm_second_frame() {
        // One cell per row keeps each request in its own run so we can
        // measure three independent cache lookups; one row of contiguous
        // same-style cells would collapse to a single run, hiding whether
        // the cache fired three times or once.
        let theme = Theme::blank();
        let cells = ascii_cells_3();
        let mut source = FakeFrame::new(1, 3, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let warmup_calls = backend.shape_calls;
        assert_eq!(warmup_calls, 3, "frame 1 must shape every distinct run");
        assert_eq!(builder.shape_cache_stats().misses, 3);
        assert_eq!(builder.shape_cache_stats().hits, 0);

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(
            backend.shape_calls, warmup_calls,
            "frame 2 must hit the cache for every run — no new backend calls"
        );
        assert_eq!(builder.shape_cache_stats().hits, 3);
    }

    #[test]
    fn shape_cache_repopulates_after_reset_glyphs() {
        let theme = Theme::blank();
        let cells = ascii_cells_3();
        let mut source = FakeFrame::new(1, 3, &cells);
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
        // Same text under different colors must hit the cache — the key
        // omits color. One cell per row keeps each "X" in its own run.
        let theme = Theme::blank();
        let warm = [
            ("X", CellColor::Rgb(255, 0, 0), CellColor::Default, plain()),
            ("X", CellColor::Rgb(0, 255, 0), CellColor::Default, plain()),
            ("X", CellColor::Rgb(0, 0, 255), CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(1, 3, &warm);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 1);
        assert_eq!(builder.shape_cache_stats().hits, 2);
        assert_eq!(builder.shape_cache_stats().misses, 1);
    }

    #[test]
    fn shape_cache_distinguishes_bold_and_italic() {
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            ("a", CellColor::Default, CellColor::Default, bold()),
            ("a", CellColor::Default, CellColor::Default, italic()),
            ("a", CellColor::Default, CellColor::Default, plain()),
        ];
        // Differing attrs already split the run, so a single-row layout
        // exercises the cache key correctly.
        let mut source = FakeFrame::new(4, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        // 3 unique (text, attrs) keys: plain, bold, italic. The fourth
        // cell repeats `plain` so it hits.
        assert_eq!(backend.shape_calls, 3);
        assert_eq!(builder.shape_cache_stats().hits, 1);
        assert_eq!(builder.shape_cache_stats().misses, 3);
    }

    #[test]
    fn shape_cache_warm_hit_rate_above_95_percent() {
        // 80×24 of repeating ASCII printable chars with ~10% bold. Working
        // set ≈ 384 unique runs after grouping — comfortably under cache
        // capacity (2048 slots), so steady-state hit rate is near 100%.
        let theme = Theme::blank();
        let texts: Vec<String> = (0..(80 * 24))
            .map(|i| {
                let c = (b'!' + (i % 94) as u8) as char;
                c.to_string()
            })
            .collect();
        let cells: Vec<FakeCell> = texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let attrs = if i % 11 == 0 { bold() } else { plain() };
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

    // ────────────────────────────────────────────────────────────────────
    // Procedural codepoint interception (issue #27)
    // ────────────────────────────────────────────────────────────────────

    #[test]
    fn box_drawing_cells_bypass_shape_runs_and_land_in_procedural_buffer() {
        // ─ (U+2500) is procedural; the visitor should not include it in any
        // shape run, but it should appear in the procedural-cell buffer.
        let theme = Theme::blank();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain()),
            ("─", CellColor::Default, CellColor::Default, plain()),
            ("b", CellColor::Default, CellColor::Default, plain()),
        ];
        let (_bg, runs, procedural) = collect_runs_with_procedural(3, 1, &cells, &theme);

        assert_eq!(runs.len(), 2, "procedural cell must break the run");
        assert_eq!(runs[0].text, "a");
        assert_eq!(runs[1].text, "b");
        assert_eq!(procedural.len(), 1);
        assert_eq!(procedural[0].codepoint, '─');
        assert_eq!(procedural[0].col, 1);
        assert_eq!(procedural[0].row, 0);
    }

    #[test]
    fn procedural_cells_emit_one_celltext_each_without_shaping() {
        let theme = Theme::blank();
        // ┌─┐ — three procedural codepoints, no shaping needed.
        let cells = [
            ("┌", CellColor::Default, CellColor::Default, plain()),
            ("─", CellColor::Default, CellColor::Default, plain()),
            ("┐", CellColor::Default, CellColor::Default, plain()),
        ];
        let mut source = FakeFrame::new(3, 1, &cells);
        let mut backend = distributing_backend();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));

        // No characters reach the shape backend.
        assert_eq!(builder.shape_cache_stats().misses, 0);
        // Each of the three cells produces a CellText anchored at its
        // column.
        let cols: Vec<u16> = builder.text_cells().iter().map(|c| c.grid_pos[0]).collect();
        assert_eq!(cols, vec![0, 1, 2]);
    }

    #[test]
    fn procedural_codepoint_is_rejected_when_paired_with_combining_mark() {
        // A cell whose grapheme is more than one codepoint must not be
        // claimed by the procedural pipeline, even if its first char is in
        // the box-drawing range. (Combining marks attached to box-drawing
        // are exotic but the safety net should hold.)
        let theme = Theme::blank();
        let combined = "─\u{0301}"; // ─ + combining acute
        let cells = [(combined, CellColor::Default, CellColor::Default, plain())];
        let (_bg, runs, procedural) = collect_runs_with_procedural(1, 1, &cells, &theme);

        assert!(procedural.is_empty());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, combined);
    }

    #[test]
    fn procedural_pipeline_preserves_min_contrast_for_default_fg_only() {
        let theme = Theme::blank();
        let cells = [
            ("─", CellColor::Default, CellColor::Default, plain()),
            ("─", CellColor::Rgb(10, 20, 30), CellColor::Default, plain()),
        ];
        let (_bg, _runs, procedural) = collect_runs_with_procedural(2, 1, &cells, &theme);

        assert_eq!(procedural.len(), 2);
        assert!(procedural[0].min_contrast, "default fg gets min-contrast");
        assert!(
            !procedural[1].min_contrast,
            "explicit RGB fg skips min-contrast"
        );
    }

    #[test]
    fn reset_glyphs_clears_procedural_atlas_slots() {
        // After reset_glyphs (e.g. font-size change), a second frame must
        // re-rasterize the procedural glyph because the bitmap was sized to
        // the old metrics.
        let theme = Theme::blank();
        let cells = [("─", CellColor::Default, CellColor::Default, plain())];
        let mut source = FakeFrame::new(1, 1, &cells);
        let mut backend = distributing_backend();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(builder.text_cells().len(), 1);
        let first = builder.text_cells()[0];

        builder.reset_glyphs();
        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(builder.text_cells().len(), 1);
        let second = builder.text_cells()[0];

        // The atlas was reset, so the same codepoint should be re-allocated
        // and land at the start of the grayscale plane again.
        assert_eq!(second.glyph_pos, first.glyph_pos);
        assert_eq!(second.glyph_size, first.glyph_size);
    }
}
