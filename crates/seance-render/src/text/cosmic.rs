//! cosmic-text implementation of [`TextBackend`].
//!
//! Wraps [`cosmic_text::FontSystem`] (shaping + fontdb discovery) and
//! [`SwashCache`] (rasterization). Sole crate-local user of
//! `cosmic_text`; swapping to parley or a hand-rolled stack is a
//! file-local change.

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FeatureTag, FontFeatures, FontSystem, Metrics, Shaping, Style,
    SwashCache, SwashContent, Weight, fontdb::Query,
};
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;

use super::backend::{
    CellMetrics, FontAttrs, GlyphFormat, GlyphId, RasterizedGlyph, RunGlyph, ShapedGlyph,
    TextBackend,
};

const FALLBACK_LINE_HEIGHT_SCALE: f32 = 1.2;

#[derive(Debug, Clone, Copy, PartialEq)]
enum MetricModifier {
    Percent(f32),
    Absolute(i32),
}

#[derive(Debug, Clone, Copy)]
struct FaceGridMetrics {
    face_width: f32,
    face_height: f32,
    face_baseline_from_bottom: f32,
}

pub struct CosmicTextBackend {
    fs: FontSystem,
    swash: SwashCache,
    metrics: CellMetrics,
    font_size: f32,
    scale: f64,
    family: String,
    adjust_cell_height: Option<MetricModifier>,
    /// Pre-parsed OpenType feature tags applied to every shape_run call.
    /// `Vec<FeatureTag>` rather than `FontFeatures` because we materialize
    /// a fresh `FontFeatures` per shape (cheap — small Vec) so we can mix
    /// per-style additions if those ever land.
    font_features: Vec<FeatureTag>,

    /// Intern CacheKey → stable `GlyphId` so the atlas cache can key on
    /// a small integer without knowing the cosmic-text encoding.
    key_to_id: HashMap<CacheKey, GlyphId, FxBuildHasher>,
    id_to_key: Vec<CacheKey>,
}

impl CosmicTextBackend {
    pub fn new(
        family: &str,
        font_size: f32,
        scale: f64,
        adjust_cell_height: Option<&str>,
        font_features: &[String],
    ) -> Self {
        let mut fs = FontSystem::new();
        let adjust_cell_height = parse_metric_modifier(adjust_cell_height);
        let metrics = compute_metrics(&mut fs, family, font_size, scale, adjust_cell_height);
        Self {
            fs,
            swash: SwashCache::new(),
            metrics,
            font_size,
            scale,
            family: family.to_string(),
            adjust_cell_height,
            font_features: parse_feature_tags(font_features),
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

    fn refresh_metrics(&mut self) {
        self.metrics = compute_metrics(
            &mut self.fs,
            &self.family,
            self.font_size,
            self.scale,
            self.adjust_cell_height,
        );
    }
}

impl TextBackend for CosmicTextBackend {
    fn metrics(&self) -> &CellMetrics {
        &self.metrics
    }

    fn set_font_size(&mut self, points: f32) {
        self.font_size = points;
        self.refresh_metrics();
        self.key_to_id.clear();
        self.id_to_key.clear();
    }

    fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.refresh_metrics();
        self.key_to_id.clear();
        self.id_to_key.clear();
    }

    fn set_adjust_cell_height(&mut self, value: Option<&str>) {
        self.adjust_cell_height = parse_metric_modifier(value);
        self.refresh_metrics();
    }

    fn set_font_features(&mut self, features: &[String]) {
        self.font_features = parse_feature_tags(features);
    }

    fn shape_run(&mut self, text: &str, attrs: FontAttrs, out: &mut Vec<RunGlyph>) {
        if text.is_empty() {
            return;
        }
        let scaled = self.scaled_font_size();
        let cosmic_metrics = Metrics::new(scaled, self.metrics.cell_height);
        let attrs = make_attrs(&self.family, &self.font_features, attrs);

        let mut buffer = Buffer::new(&mut self.fs, cosmic_metrics);
        buffer.set_text(&mut self.fs, text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.fs, false);

        // Two-step: gather (cluster, CacheKey) before interning so the
        // borrow on `buffer.layout_runs()` ends before we touch `self`.
        let pairs: Vec<(u16, CacheKey)> = buffer
            .layout_runs()
            .flat_map(|run| {
                run.glyphs.iter().map(move |g| {
                    let key = CacheKey::new(
                        g.font_id,
                        g.glyph_id,
                        g.font_size,
                        (0.0, 0.0),
                        g.font_weight,
                        g.cache_key_flags,
                    )
                    .0;
                    let cluster = u16::try_from(g.start).unwrap_or(u16::MAX);
                    (cluster, key)
                })
            })
            .collect();

        out.extend(pairs.into_iter().map(|(cluster, key)| RunGlyph {
            glyph: ShapedGlyph {
                id: self.intern(key),
            },
            cluster,
        }));
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

/// Build a cosmic-text [`Attrs`] tied to `family`'s lifetime. Returning
/// from a `&self` method instead would broaden the borrow to all of
/// `self`, blocking the `&mut self.fs` borrow that `shape_run` needs.
fn make_attrs<'a>(family: &'a str, features: &[FeatureTag], attrs: FontAttrs) -> Attrs<'a> {
    let mut font_features = FontFeatures::new();
    for tag in features {
        font_features.enable(*tag);
    }
    Attrs::new()
        .family(Family::Name(family))
        .weight(if attrs.bold {
            Weight::BOLD
        } else {
            Weight::NORMAL
        })
        .style(if attrs.italic {
            Style::Italic
        } else {
            Style::Normal
        })
        .font_features(font_features)
}

fn parse_feature_tags(features: &[String]) -> Vec<FeatureTag> {
    let mut out = Vec::with_capacity(features.len());
    for raw in features {
        let bytes = raw.as_bytes();
        if let Ok(tag) = <&[u8; 4]>::try_from(bytes) {
            out.push(FeatureTag::new(tag));
        } else {
            log::warn!(
                "ignoring font.features entry {raw:?} — OpenType tags must be exactly 4 ASCII bytes"
            );
        }
    }
    out
}

fn parse_metric_modifier(value: Option<&str>) -> Option<MetricModifier> {
    let input = value?.trim();
    if input.is_empty() {
        return None;
    }

    if let Some(percent) = input.strip_suffix('%') {
        let Ok(percent) = percent.parse::<f32>() else {
            log::warn!("invalid adjust_cell_height value: {input}");
            return None;
        };
        return Some(MetricModifier::Percent((1.0 + percent / 100.0).max(0.0)));
    }

    match input.parse::<i32>() {
        Ok(value) => Some(MetricModifier::Absolute(value)),
        Err(_) => {
            log::warn!("invalid adjust_cell_height value: {input}");
            None
        }
    }
}

fn apply_metric_modifier(value: u32, modifier: MetricModifier) -> u32 {
    match modifier {
        MetricModifier::Percent(percent) => ((value as f32) * percent).round().max(0.0) as u32,
        MetricModifier::Absolute(delta) => {
            (i64::from(value) + i64::from(delta)).clamp(0, i64::from(u32::MAX)) as u32
        }
    }
}

fn face_grid_metrics(
    fs: &mut FontSystem,
    family: &str,
    scaled_font_size: f32,
) -> Option<FaceGridMetrics> {
    let attrs = Attrs::new().family(Family::Name(family));
    let families = [attrs.family];
    let query = Query {
        families: &families,
        weight: attrs.weight,
        stretch: attrs.stretch,
        style: attrs.style,
    };
    let id = fs.db().query(&query)?;
    let font = fs.get_font(id, attrs.weight)?;
    let metrics = font.metrics();
    let units_per_em = f32::from(metrics.units_per_em).max(1.0);
    let font_scale = scaled_font_size / units_per_em;

    let face_width = font
        .monospace_em_width()
        .map(|em_width| em_width * scaled_font_size)
        .filter(|width| *width > 0.0)
        .unwrap_or_else(|| measure_cell_width(fs, family, scaled_font_size));
    let ascent = metrics.ascent * font_scale;
    let descent = metrics.descent * font_scale;
    let leading = metrics.leading * font_scale;

    Some(FaceGridMetrics {
        face_width,
        face_height: (ascent - descent + leading).max(1.0),
        face_baseline_from_bottom: leading / 2.0 - descent,
    })
}

fn fallback_face_grid_metrics(
    fs: &mut FontSystem,
    family: &str,
    scaled_font_size: f32,
) -> FaceGridMetrics {
    let line_height = (scaled_font_size * FALLBACK_LINE_HEIGHT_SCALE).max(1.0);
    let (face_width, baseline_from_top) =
        measure_cell_width_and_baseline(fs, family, scaled_font_size, line_height);
    FaceGridMetrics {
        face_width,
        face_height: line_height,
        face_baseline_from_bottom: line_height - baseline_from_top,
    }
}

fn measure_cell_width(fs: &mut FontSystem, family: &str, scaled_font_size: f32) -> f32 {
    measure_cell_width_and_baseline(fs, family, scaled_font_size, scaled_font_size.max(1.0)).0
}

fn measure_cell_width_and_baseline(
    fs: &mut FontSystem,
    family: &str,
    scaled_font_size: f32,
    line_height: f32,
) -> (f32, f32) {
    let attrs = Attrs::new().family(Family::Name(family));
    let mut buffer = Buffer::new(fs, Metrics::new(scaled_font_size, line_height));
    buffer.set_text(fs, "M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(fs, false);

    match buffer.layout_runs().next() {
        Some(run) => {
            let width = run
                .glyphs
                .iter()
                .next()
                .map(|glyph| glyph.w)
                .unwrap_or(scaled_font_size * 0.6)
                .max(1.0);
            (width, run.line_y)
        }
        None => ((scaled_font_size * 0.6).max(1.0), line_height * 0.8),
    }
}

fn cell_metrics_from_face(
    face: FaceGridMetrics,
    adjust_cell_height: Option<MetricModifier>,
) -> CellMetrics {
    let cell_width = face.face_width.round().max(1.0);
    let base_cell_height = face.face_height.round().max(1.0) as u32;
    let cell_height = adjust_cell_height
        .map(|modifier| apply_metric_modifier(base_cell_height, modifier))
        .unwrap_or(base_cell_height)
        .max(1) as f32;
    let baseline_from_bottom =
        (face.face_baseline_from_bottom - (cell_height - face.face_height) / 2.0).round();
    let baseline = (cell_height - baseline_from_bottom).clamp(0.0, cell_height);

    CellMetrics {
        cell_width,
        cell_height,
        baseline,
    }
}

/// Compute Ghostty-like cell metrics from the font's face metrics where
/// possible, falling back to a simple shaped-glyph estimate if fontdb lookup
/// fails.
fn compute_metrics(
    fs: &mut FontSystem,
    family: &str,
    font_size: f32,
    scale: f64,
    adjust_cell_height: Option<MetricModifier>,
) -> CellMetrics {
    let scaled_font_size = font_size * scale as f32;
    let face = face_grid_metrics(fs, family, scaled_font_size)
        .unwrap_or_else(|| fallback_face_grid_metrics(fs, family, scaled_font_size));
    cell_metrics_from_face(face, adjust_cell_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metric_modifier_like_ghostty() {
        assert_eq!(
            parse_metric_modifier(Some("20%")),
            Some(MetricModifier::Percent(1.2))
        );
        assert_eq!(
            parse_metric_modifier(Some("-20%")),
            Some(MetricModifier::Percent(0.8))
        );
        assert_eq!(
            parse_metric_modifier(Some("-100%")),
            Some(MetricModifier::Percent(0.0))
        );
        assert_eq!(
            parse_metric_modifier(Some("3")),
            Some(MetricModifier::Absolute(3))
        );
        assert_eq!(parse_metric_modifier(Some("")), None);
    }

    #[test]
    fn applies_metric_modifier_with_ghostty_rounding() {
        assert_eq!(apply_metric_modifier(18, MetricModifier::Percent(1.2)), 22);
        assert_eq!(apply_metric_modifier(18, MetricModifier::Percent(0.8)), 14);
        assert_eq!(apply_metric_modifier(18, MetricModifier::Absolute(3)), 21);
        assert_eq!(apply_metric_modifier(18, MetricModifier::Absolute(-30)), 0);
    }

    #[test]
    fn parse_feature_tags_keeps_4byte_ascii_and_drops_others() {
        let parsed = parse_feature_tags(&[
            "calt".to_string(),
            "liga".to_string(),
            "ss01".to_string(),
            // wrong length
            "lig".to_string(),
            "stylistic".to_string(),
            String::new(),
        ]);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].as_bytes(), b"calt");
        assert_eq!(parsed[1].as_bytes(), b"liga");
        assert_eq!(parsed[2].as_bytes(), b"ss01");
    }

    #[test]
    fn cell_metrics_center_face_after_height_adjustment() {
        let metrics = cell_metrics_from_face(
            FaceGridMetrics {
                face_width: 8.6,
                face_height: 18.2,
                face_baseline_from_bottom: 14.0,
            },
            Some(MetricModifier::Percent(1.2)),
        );

        assert_eq!(metrics.cell_width, 9.0);
        assert_eq!(metrics.cell_height, 22.0);
        assert_eq!(metrics.baseline, 10.0);
    }
}
