//! cosmic-text implementation of [`TextBackend`].
//!
//! Wraps [`cosmic_text::FontSystem`] (shaping + font discovery via
//! fontdb) and [`SwashCache`] (rasterization). The only crate-local
//! site that imports `cosmic_text`; swapping to parley or a hand-
//! rolled stack is a file-local change.

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, SwashCache,
    SwashContent,
};
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;

use super::backend::{
    CellMetrics, GlyphFormat, GlyphId, RasterizedGlyph, ShapedGlyph, TextBackend,
};

pub struct CosmicTextBackend {
    fs: FontSystem,
    swash: SwashCache,
    metrics: CellMetrics,
    font_size: f32,
    scale: f64,
    family: String,

    /// Intern CacheKey → stable GlyphId so the caller's atlas cache
    /// can key on `GlyphId` without knowing the cosmic-text encoding.
    key_to_id: HashMap<CacheKey, GlyphId, FxBuildHasher>,
    id_to_key: HashMap<GlyphId, CacheKey, FxBuildHasher>,
    next_id: u64,
}

impl CosmicTextBackend {
    pub fn new(family: &str, font_size: f32, scale: f64) -> Self {
        let mut fs = FontSystem::new();
        let swash = SwashCache::new();
        let metrics = compute_metrics(&mut fs, family, font_size, scale);
        Self {
            fs,
            swash,
            metrics,
            font_size,
            scale,
            family: family.to_string(),
            key_to_id: HashMap::with_hasher(FxBuildHasher),
            id_to_key: HashMap::with_hasher(FxBuildHasher),
            next_id: 0,
        }
    }

    fn recompute_metrics(&mut self) {
        self.metrics = compute_metrics(&mut self.fs, &self.family, self.font_size, self.scale);
    }

    fn reset_intern(&mut self) {
        self.key_to_id.clear();
        self.id_to_key.clear();
        self.next_id = 0;
    }

    fn intern(&mut self, key: CacheKey) -> GlyphId {
        if let Some(&id) = self.key_to_id.get(&key) {
            return id;
        }
        let id = GlyphId(self.next_id);
        self.next_id += 1;
        self.key_to_id.insert(key, id);
        self.id_to_key.insert(id, key);
        id
    }
}

impl TextBackend for CosmicTextBackend {
    fn metrics(&self) -> &CellMetrics {
        &self.metrics
    }

    fn set_font_size(&mut self, points: f32) {
        self.font_size = points;
        self.recompute_metrics();
        self.reset_intern();
    }

    fn shape_cell(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        if text.is_empty() {
            return;
        }
        let scaled_size = self.font_size * self.scale as f32;
        let cosmic_metrics = Metrics::new(scaled_size, self.metrics.cell_height);
        let attrs = Attrs::new().family(Family::Name(&self.family));

        let mut buffer = Buffer::new(&mut self.fs, cosmic_metrics);
        buffer.set_text(&mut self.fs, text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);

        let mut shaped_glyphs: Vec<ShapedGlyph> = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                let (key, _, _) = CacheKey::new(
                    glyph.font_id,
                    glyph.glyph_id,
                    scaled_size,
                    (0.0, 0.0),
                    cosmic_text::fontdb::Weight::NORMAL,
                    CacheKeyFlags::empty(),
                );
                shaped_glyphs.push(ShapedGlyph {
                    id: self.intern(key),
                });
            }
        }
        out.extend(shaped_glyphs);
    }

    fn rasterize(&mut self, glyph: GlyphId) -> Option<RasterizedGlyph> {
        let key = *self.id_to_key.get(&glyph)?;
        let image = self.swash.get_image(&mut self.fs, key).as_ref()?;
        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }
        let format = match image.content {
            SwashContent::Color => GlyphFormat::Color,
            _ => GlyphFormat::Alpha,
        };
        Some(RasterizedGlyph {
            data: image.data.clone(),
            width: image.placement.width,
            height: image.placement.height,
            bearing_x: image.placement.left,
            bearing_y: image.placement.top,
            format,
        })
    }
}

fn compute_metrics(fs: &mut FontSystem, family: &str, font_size: f32, scale: f64) -> CellMetrics {
    let scaled_size = font_size * scale as f32;
    let line_height = (scaled_size * 1.2).ceil();
    let cosmic_metrics = Metrics::new(scaled_size, line_height);

    let attrs = Attrs::new().family(Family::Name(family));

    let mut buffer = Buffer::new(fs, cosmic_metrics);
    buffer.set_text(fs, "M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);

    let mut cell_width = scaled_size * 0.6;
    if let Some(run) = buffer.layout_runs().next()
        && let Some(glyph) = run.glyphs.iter().next()
    {
        cell_width = glyph.w;
    }

    let cell_width = cell_width.ceil();
    let cell_height = line_height;

    CellMetrics {
        cell_width,
        cell_height,
    }
}
