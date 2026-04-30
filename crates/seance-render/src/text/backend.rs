//! The text-shaping / rasterization seam.
//!
//! [`TextBackend`] is the single abstraction that would move if we
//! swapped cosmic-text for parley or a hand-rolled rustybuzz+swash
//! stack. The atlas is **not** behind this trait: it is a
//! backend-agnostic data structure that the caller passes in.
//!
//! The two-step design (shape, then rasterize) lets the caller hold a
//! per-glyph atlas cache: once a glyph is in the atlas the second
//! step is skipped. See [`crate::text::cell_builder`] for the driver.

/// Per-cell geometry the renderer uses for layout.
///
/// `baseline` is the y-offset from the top of the cell to the glyph
/// baseline. Centering the baseline within the line-box (rather than
/// placing it at the cell bottom) keeps descenders inside the cell and
/// splits the leading evenly above and below the ink.
#[derive(Debug, Clone, Copy)]
pub struct CellMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub baseline: f32,
}

/// Opaque identifier for a shaped glyph at a given size.
///
/// The encoding is backend-private; the caller just needs
/// `Hash + Eq + Copy` for the atlas cache. Backends typically pack
/// (font id, glyph id, scaled size) into the `u64`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlyphId(pub u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FontAttrs {
    pub bold: bool,
    pub italic: bool,
}

/// A shaped glyph produced by [`TextBackend::shape_run`].
///
/// `cluster` is the byte offset within the run input where the source
/// cluster (one or more codepoints, possibly joined by ZWJ) starts.
/// Multiple glyphs may share a cluster (mark + base, multi-glyph
/// fallback) and a single glyph may cover several clusters
/// (ligatures, regional flag pairs).
#[derive(Debug, Clone, Copy)]
pub struct ShapedGlyph {
    pub id: GlyphId,
    pub cluster: u32,
}

/// Whether a rasterized glyph is a grayscale alpha mask or a full
/// color bitmap (emoji, icon fonts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphFormat {
    Alpha,
    Color,
}

/// A rasterized glyph ready for atlas insertion.
pub struct RasterizedGlyph {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub format: GlyphFormat,
}

/// Text shaping + rasterization.
///
/// Sole seam where the implementation can change without touching the
/// GPU pipeline or the VT iteration. The sole implementor today is
/// [`crate::text::cosmic::CosmicTextBackend`].
pub trait TextBackend {
    fn metrics(&self) -> &CellMetrics;

    fn set_font_size(&mut self, points: f32);

    fn set_scale(&mut self, scale: f64);

    fn set_adjust_cell_height(&mut self, value: Option<&str>);

    fn set_adjust_cell_width(&mut self, value: Option<&str>);

    /// Replace the active OpenType feature list ("calt", "liga", "ss01", …).
    /// Callers must drop their shape caches; the same `(text, attrs)` key
    /// can map to different glyphs once features change.
    fn set_features(&mut self, features: &[String]);

    /// Replace the fallback family list. Callers must drop their shape
    /// caches.
    fn set_fallback(&mut self, fallback: &[String]);

    /// Shape a contiguous run of text into glyphs. Each emitted glyph
    /// carries the byte offset of its source cluster within `text`, so
    /// callers driving multi-cell input (ligatures, ZWJ sequences, flag
    /// pairs) can attribute composed glyphs back to the originating
    /// cell. Appends to `out`.
    fn shape_run(&mut self, text: &str, attrs: FontAttrs, out: &mut Vec<ShapedGlyph>);

    /// Rasterize a previously shaped glyph. Returns `None` for zero-
    /// sized glyphs (whitespace etc.) — callers should skip those.
    fn rasterize(&mut self, glyph: GlyphId) -> Option<RasterizedGlyph>;
}
