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
    shape_cache: ShapeCache,
    bg_cells: Vec<[u8; 4]>,
    text_cells: Vec<CellText>,
    requests: Vec<CellRequest>,
    shape_scratch: Vec<ShapedGlyph>,
    /// Reused per-run scratch — concatenated text and a cursor of byte
    /// offsets into that text, one per cell in the run, plus a sentinel
    /// `text.len()` at the tail. Built fresh for every run; held on the
    /// builder to avoid reallocating on each frame.
    run_text: String,
    run_cell_starts: Vec<u32>,
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
            run_text: String::new(),
            run_cell_starts: Vec::new(),
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
            &mut ShapePass {
                atlas: &mut self.atlas,
                glyph_slots: &mut self.glyph_slots,
                shape_cache: &mut self.shape_cache,
                shape_scratch: &mut self.shape_scratch,
                run_text: &mut self.run_text,
                run_cell_starts: &mut self.run_cell_starts,
                out: &mut self.text_cells,
            },
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

/// Per-frame mutable state `shape_and_pack` consumes. Bundled because
/// individually-borrowed buffers would otherwise blow past clippy's
/// `too_many_arguments` ceiling once the per-run text and cluster
/// scratch were added.
struct ShapePass<'a> {
    atlas: &'a mut GlyphAtlas,
    glyph_slots: &'a mut GlyphSlots,
    shape_cache: &'a mut ShapeCache,
    shape_scratch: &'a mut Vec<ShapedGlyph>,
    run_text: &'a mut String,
    run_cell_starts: &'a mut Vec<u32>,
    out: &'a mut Vec<CellText>,
}

/// Text-aware, VT-free pass: group contiguous same-style cells into
/// shape runs, shape (with cache), cache glyphs, emit instance records.
///
/// Multi-cell shaping is required to render ligatures (`==`, `=>`,
/// `!=`, …), regional-indicator flag pairs, ZWJ sequences (skin-tone
/// emoji, family glyphs), and combining marks: harfbuzz can only
/// compose a glyph when it sees the whole cluster. Anchoring each
/// emitted glyph at the column of its source cluster keeps the GPU
/// layout in lock-step with the VT grid.
fn shape_and_pack(
    requests: &[CellRequest],
    backend: &mut dyn TextBackend,
    pass: &mut ShapePass<'_>,
) {
    let mut start = 0;
    while start < requests.len() {
        let end = run_end(requests, start);
        build_run_text(&requests[start..end], pass.run_text, pass.run_cell_starts);
        shape_with_cache(
            pass.shape_cache,
            backend,
            pass.run_text,
            requests[start].font_attrs,
            pass.shape_scratch,
        );
        for glyph in &*pass.shape_scratch {
            let Some(entry) = ensure_glyph_slot(pass.glyph_slots, pass.atlas, backend, glyph.id)
            else {
                continue;
            };
            let cell_idx = cluster_to_cell(pass.run_cell_starts, glyph.cluster);
            let req = &requests[start + cell_idx];
            pass.out.push(CellText {
                glyph_pos: entry.pos,
                glyph_size: entry.size,
                bearings: [entry.bearing_x as i16, entry.bearing_y as i16],
                grid_pos: [req.col, req.row],
                color: req.fg,
                atlas_and_flags: u32::from(entry.is_color),
            });
        }
        start = end;
    }
}

/// Largest `end` such that `requests[start..end]` is a single shaping
/// run: same row, same `font_attrs`, columns strictly contiguous (no
/// gap from an empty cell, since `walk_grid` skips emit-time-empty
/// cells entirely).
fn run_end(requests: &[CellRequest], start: usize) -> usize {
    let head = &requests[start];
    let mut end = start + 1;
    while end < requests.len() {
        let next = &requests[end];
        if next.row != head.row
            || next.font_attrs != head.font_attrs
            || next.col != requests[end - 1].col + 1
        {
            break;
        }
        end += 1;
    }
    end
}

/// Concatenate `run`'s cell texts into `text`, recording byte offsets
/// where each cell starts. The trailing entry equals `text.len()` so
/// [`cluster_to_cell`] can binary-search without bounds gymnastics.
fn build_run_text(run: &[CellRequest], text: &mut String, cell_starts: &mut Vec<u32>) {
    text.clear();
    cell_starts.clear();
    cell_starts.reserve(run.len() + 1);
    for cell in run {
        cell_starts.push(text.len() as u32);
        text.push_str(&cell.text);
    }
    cell_starts.push(text.len() as u32);
}

/// Map a glyph's source-cluster byte offset to the cell index within
/// its run. The largest `i` whose `cell_starts[i] <= cluster` is the
/// originating cell; ligatures with `cluster = 0` always anchor at the
/// run's first cell, matching Ghostty's anchor-at-cluster-start rule.
fn cluster_to_cell(cell_starts: &[u32], cluster: u32) -> usize {
    debug_assert!(cell_starts.len() >= 2);
    let inner = &cell_starts[..cell_starts.len() - 1];
    match inner.binary_search(&cluster) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
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
            // Deterministic per-character glyph result keyed off the
            // codepoint so cache round-trips and run-grouping are both
            // observable. Each char becomes one glyph at its byte
            // offset so tests can verify cluster mapping.
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
        // One cell per row keeps each request in its own run so the
        // per-run cache hits we measure are meaningful (a single row of
        // contiguous same-style cells would collapse to one run anyway,
        // hiding whether the cache fired three times or once).
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
        // Same text under different theme colors must still hit the
        // cache — the key omits color. Guards against a refactor
        // that bakes color into the key. One cell per row keeps the
        // three "X"s in separate runs (otherwise they collapse to a
        // single "XXX" run and the test no longer probes the key).
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
        // Differing attrs already split the run, so a single-row layout
        // exercises the cache key correctly.
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
    fn run_grouping_collapses_contiguous_same_style_cells() {
        // 5 contiguous plain cells on row 0 must shape as one run.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            ("h", CellColor::Default, CellColor::Default, plain),
            ("e", CellColor::Default, CellColor::Default, plain),
            ("l", CellColor::Default, CellColor::Default, plain),
            ("l", CellColor::Default, CellColor::Default, plain),
            ("o", CellColor::Default, CellColor::Default, plain),
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
        // [plain plain bold plain] → [plain plain] | [bold] | [plain]
        // = 3 runs. Different attrs cannot share a run because cosmic-text
        // shapes one (text, attrs) at a time.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let bold = CellAttrs {
            bold: true,
            ..CellAttrs::default()
        };
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain),
            ("b", CellColor::Default, CellColor::Default, plain),
            ("c", CellColor::Default, CellColor::Default, bold),
            ("d", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(4, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 3);
    }

    #[test]
    fn run_grouping_breaks_on_empty_cell_gap() {
        // An empty middle cell never reaches `shape_and_pack` (walk_grid
        // skips empty cells), so the contiguity check splits the run.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain),
            ("b", CellColor::Default, CellColor::Default, plain),
            ("", CellColor::Default, CellColor::Default, plain),
            ("c", CellColor::Default, CellColor::Default, plain),
            ("d", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(5, 1, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 2);
    }

    #[test]
    fn run_grouping_breaks_across_rows() {
        // Same style, but different rows means different runs — a
        // ligature cannot span a line break.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            ("a", CellColor::Default, CellColor::Default, plain),
            ("b", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(1, 2, &cells);
        let mut backend = StubBackend::new();
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        assert_eq!(backend.shape_calls, 2);
    }

    #[test]
    fn cluster_to_cell_anchors_glyphs_at_their_source_cells() {
        // The stub backend emits one glyph per char with cluster set to
        // the byte offset, so a 3-cell ASCII run must produce three
        // CellTexts at cols 0, 1, 2 — proving the cluster→cell mapping
        // distributes glyphs back to the originating column rather than
        // piling them onto the run anchor.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            ("X", CellColor::Default, CellColor::Default, plain),
            ("Y", CellColor::Default, CellColor::Default, plain),
            ("Z", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(3, 1, &cells);
        // Backend that returns a non-zero atlas entry — needed so the
        // CellText emit isn't suppressed by the rasterize→None path.
        struct OneGlyphBackend {
            metrics: CellMetrics,
            shape_calls: u32,
        }
        impl TextBackend for OneGlyphBackend {
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
                self.shape_calls += 1;
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
        let mut backend = OneGlyphBackend {
            metrics: CellMetrics {
                cell_width: 10.0,
                cell_height: 20.0,
                baseline: 16.0,
            },
            shape_calls: 0,
        };
        let mut builder = CellBuilder::new();

        builder.build_frame(&mut source, &mut backend, build_config(&theme));
        let cols: Vec<u16> = builder.text_cells().iter().map(|c| c.grid_pos[0]).collect();
        assert_eq!(cols, vec![0, 1, 2]);
        assert_eq!(backend.shape_calls, 1);
    }

    #[test]
    fn ligature_glyph_anchors_at_cluster_zero() {
        // Two-cell run "==" with a backend that emits a single ligature
        // glyph at cluster 0 — the produced CellText must land on the
        // first cell's column. This is the multi-cell shaping path that
        // ligatures rely on.
        let theme = Theme::blank();
        let plain = CellAttrs::default();
        let cells = [
            ("=", CellColor::Default, CellColor::Default, plain),
            ("=", CellColor::Default, CellColor::Default, plain),
        ];
        let mut source = FakeFrame::new(2, 1, &cells);
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

    #[test]
    fn cluster_to_cell_handles_in_range_lookups() {
        // 3 cells starting at byte offsets 0, 1, 3 (cell 1 is "é" = 2
        // bytes), with a sentinel of 4 at the tail.
        let cell_starts = [0u32, 1, 3, 4];
        assert_eq!(cluster_to_cell(&cell_starts, 0), 0);
        assert_eq!(cluster_to_cell(&cell_starts, 1), 1);
        assert_eq!(
            cluster_to_cell(&cell_starts, 2),
            1,
            "mid-cluster falls into the cell that opened it"
        );
        assert_eq!(cluster_to_cell(&cell_starts, 3), 2);
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
