use cosmic_text::{
    Attrs, Buffer, Family, FontSystem as CosmicFontSystem, Metrics, Shaping, SwashCache,
};

pub struct CellMetrics {
    pub cell_width: f32,
    pub cell_height: f32,
    pub baseline: f32,
}

pub struct FontSystem {
    pub(super) inner: CosmicFontSystem,
    pub(super) swash: SwashCache,
    metrics: CellMetrics,
    font_size: f32,
    scale: f64,
    family: String,
}

impl FontSystem {
    pub fn new(family: &str, font_size: f32, scale: f64) -> Self {
        let mut inner = CosmicFontSystem::new();
        let swash = SwashCache::new();
        let metrics = compute_metrics(&mut inner, family, font_size, scale);
        Self {
            inner,
            swash,
            metrics,
            font_size,
            scale,
            family: family.to_string(),
        }
    }

    pub fn metrics(&self) -> &CellMetrics {
        &self.metrics
    }

    pub fn font_size(&self) -> f32 {
        self.font_size
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    pub fn set_font_size(&mut self, points: f32) {
        self.font_size = points;
        self.metrics = compute_metrics(&mut self.inner, &self.family, self.font_size, self.scale);
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.metrics = compute_metrics(&mut self.inner, &self.family, self.font_size, self.scale);
    }
}

fn compute_metrics(
    font_system: &mut CosmicFontSystem,
    family: &str,
    font_size: f32,
    scale: f64,
) -> CellMetrics {
    let scaled_size = font_size * scale as f32;
    let line_height = (scaled_size * 1.2).ceil();
    let cosmic_metrics = Metrics::new(scaled_size, line_height);

    let attrs = Attrs::new().family(Family::Name(family));

    let mut buffer = Buffer::new(font_system, cosmic_metrics);
    buffer.set_text(font_system, "M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let mut cell_width = scaled_size * 0.6;
    if let Some(run) = buffer.layout_runs().next()
        && let Some(glyph) = run.glyphs.iter().next()
    {
        cell_width = glyph.w;
    }

    let cell_width = cell_width.ceil();
    let cell_height = line_height;
    let baseline = (cell_height * 0.8).ceil();

    CellMetrics {
        cell_width,
        cell_height,
        baseline,
    }
}
