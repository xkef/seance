use std::collections::HashMap;

use cosmic_text::{CacheKey, SwashContent};
use rustc_hash::FxBuildHasher;

use super::atlas::{AtlasEntry, GlyphAtlas};
use super::system::FontSystem;

pub struct GlyphCache {
    pub(crate) font: FontSystem,
    pub(crate) atlas: GlyphAtlas,
    map: HashMap<CacheKey, AtlasEntry, FxBuildHasher>,
}

impl GlyphCache {
    pub fn new(family: &str, font_size: f32, scale: f64) -> Self {
        Self {
            font: FontSystem::new(family, font_size, scale),
            atlas: GlyphAtlas::new(),
            map: HashMap::with_hasher(FxBuildHasher),
        }
    }

    pub fn get_or_insert(&mut self, key: CacheKey) -> Option<&AtlasEntry> {
        if self.map.contains_key(&key) {
            return self.map.get(&key);
        }

        let image = self
            .font
            .swash
            .get_image(&mut self.font.inner, key)
            .as_ref()?;

        let is_color = matches!(image.content, SwashContent::Color);
        let width = image.placement.width;
        let height = image.placement.height;

        if width == 0 || height == 0 {
            return None;
        }

        let entry = self.atlas.insert(
            &image.data,
            width,
            height,
            image.placement.left,
            image.placement.top,
            is_color,
        )?;

        self.map.insert(key, entry);
        self.map.get(&key)
    }

    pub fn set_font_size(&mut self, points: f32) {
        self.font.set_font_size(points);
        self.atlas.reset();
        self.map.clear();
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.font.set_scale(scale);
        self.atlas.reset();
        self.map.clear();
    }
}
