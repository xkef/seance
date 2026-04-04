//! cosmic-text implementation of [`TextBackend`].
//!
//! Wraps [`cosmic_text::FontSystem`] (shaping + fontdb discovery) and
//! [`SwashCache`] (rasterization). Sole crate-local user of
//! `cosmic_text`; swapping to parley or a hand-rolled stack is a
//! file-local change.

use cosmic_text::{
    Attrs, Buffer, CacheKey, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, SwashCache,
    SwashContent,
};
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;

use super::backend::{
    CellMetrics, GlyphFormat, GlyphId, RasterizedGlyph, ShapedGlyph, TextBackend,
};

const LINE_HEIGHT_SCALE: f32 = 1.2;

pub struct CosmicTextBackend {
    fs: FontSystem,
    swash: SwashCache,
    metrics: CellMetrics,
    font_size: f32,
    scale: f64,
    family: String,

    /// Intern CacheKey → stable `GlyphId` so the atlas cache can key on
    /// a small integer without knowing the cosmic-text encoding.
    key_to_id: HashMap<CacheKey, GlyphId, FxBuildHasher>,
    id_to_key: Vec<CacheKey>,
}

impl CosmicTextBackend {
    pub fn new(family: &str, font_size: f32, scale: f64) -> Self {
        let mut fs = FontSystem::new();
        let metrics = compute_metrics(&mut fs, family, font_size, scale);
        Self {
            fs,
            swash: SwashCache::new(),
            metrics,
            font_size,
            scale,
            family: family.to_string(),
            key_to_id: HashMap::with_hasher(FxBuildHasher),
            id_to_key: Vec::new(),
        }
    }

    fn intern(&mut self, key: CacheKey) -> GlyphId {
        if let Some(&id) = self.key_to_id.get(&key) {
            return id;
        }
        let id = GlyphId(self.id_to_key.len() as u64);
        self.id_to_key.push(key);
        self.key_to_id.insert(key, id);
        id
    }

    fn scaled_font_size(&self) -> f32 {
        self.font_size * self.scale as f32
    }
}

impl TextBackend for CosmicTextBackend {
    fn metrics(&self) -> &CellMetrics {
        &self.metrics
    }

    fn set_font_size(&mut self, points: f32) {
        self.font_size = points;
        self.metrics = compute_metrics(&mut self.fs, &self.family, self.font_size, self.scale);
        self.key_to_id.clear();
        self.id_to_key.clear();
    }

    fn shape_cell(&mut self, text: &str, out: &mut Vec<ShapedGlyph>) {
        if text.is_empty() {
            return;
        }
        let scaled = self.scaled_font_size();
        let cosmic_metrics = Metrics::new(scaled, self.metrics.cell_height);
        let attrs = Attrs::new().family(Family::Name(&self.family));

        let mut buffer = Buffer::new(&mut self.fs, cosmic_metrics);
        buffer.set_text(&mut self.fs, text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);

        let keys: Vec<CacheKey> = buffer
            .layout_runs()
            .flat_map(|run| {
                run.glyphs.iter().map(move |g| {
                    CacheKey::new(
                        g.font_id,
                        g.glyph_id,
                        scaled,
                        (0.0, 0.0),
                        cosmic_text::fontdb::Weight::NORMAL,
                        CacheKeyFlags::empty(),
                    )
                    .0
                })
            })
            .collect();

        out.extend(keys.into_iter().map(|k| ShapedGlyph { id: self.intern(k) }));
    }

    fn rasterize(&mut self, glyph: GlyphId) -> Option<RasterizedGlyph> {
        let key = *self.id_to_key.get(glyph.0 as usize)?;
        let image = self.swash.get_image(&mut self.fs, key).as_ref()?;
        if image.placement.width == 0 || image.placement.height == 0 {
            return None;
        }
        Some(RasterizedGlyph {
            data: image.data.clone(),
            width: image.placement.width,
            height: image.placement.height,
            bearing_x: image.placement.left,
            bearing_y: image.placement.top,
            format: match image.content {
                SwashContent::Color => GlyphFormat::Color,
                _ => GlyphFormat::Alpha,
            },
        })
    }
}

/// Compute cell metrics by shaping a single "M". Width = glyph advance,
/// height = `font_size × LINE_HEIGHT_SCALE`. `baseline` comes from
/// cosmic-text's layout, which already centers ascent+descent within
/// the line-box — using it here matches that convention.
fn compute_metrics(fs: &mut FontSystem, family: &str, font_size: f32, scale: f64) -> CellMetrics {
    let scaled = font_size * scale as f32;
    let line_height = (scaled * LINE_HEIGHT_SCALE).ceil();
    let attrs = Attrs::new().family(Family::Name(family));

    let mut buffer = Buffer::new(fs, Metrics::new(scaled, line_height));
    buffer.set_text(fs, "M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);

    let (cell_width, baseline) = match buffer.layout_runs().next() {
        Some(run) => {
            let w = run
                .glyphs
                .iter()
                .next()
                .map(|g| g.w)
                .unwrap_or(scaled * 0.6)
                .ceil();
            (w, run.line_y.round())
        }
        None => (scaled.ceil() * 0.6, (line_height * 0.8).round()),
    };

    CellMetrics {
        cell_width,
        cell_height: line_height,
        baseline,
    }
}
