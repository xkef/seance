use std::sync::Arc;

use seance_config::Theme;
use seance_vt::{FrameSource, GridPos};
use winit::window::Window;

pub use crate::gpu::uniforms::CursorShape;
use crate::gpu::{CellFrame, GpuState};
use crate::text::backend::TextBackend;
use crate::text::cosmic::{BackendConfig, CosmicTextBackend};
use crate::text::{BuildFrameConfig, CellBuilder};

pub struct RendererConfig {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub font_family: String,
    pub font_size: f32,
    pub adjust_cell_height: Option<String>,
    pub adjust_cell_width: Option<String>,
    /// OpenType feature tags to enable on every shape ("calt", "liga",
    /// "ss01", …). Empty means the shaper applies its own defaults.
    pub font_features: Vec<String>,
    /// Fallback families consulted when the primary `font_family` lacks
    /// a glyph. Stored verbatim; cosmic-text already iterates through
    /// loaded fonts on miss, so the list is a hint for future wiring.
    pub font_fallback: Vec<String>,
    pub min_contrast: f32,
    /// Inner gutter between window edges and the cell grid, in physical
    /// pixels. `[x, y]`. The area outside the grid is filled by the
    /// fullscreen bg pass with the effective theme background.
    pub window_padding: [u16; 2],
    pub background_opacity: f32,
    pub theme: Theme,
}

/// Per-frame dynamic state the app supplies to the renderer.
#[derive(Debug, Clone)]
pub struct RenderInputs {
    pub vt_cursor_visible: bool,
    pub cursor_shape: CursorShape,
    pub selection: Option<(GridPos, GridPos)>,
}

impl Default for RenderInputs {
    fn default() -> Self {
        Self {
            vt_cursor_visible: true,
            cursor_shape: CursorShape::Bar,
            selection: None,
        }
    }
}

pub struct TerminalRenderer {
    backend: Box<dyn TextBackend>,
    cell_builder: CellBuilder,
    gpu: GpuState,
    theme: Theme,
    min_contrast: f32,
    background_opacity: f32,
    cell_size: [f32; 2],
    surface_width: u32,
    surface_height: u32,
    window_padding: [u16; 2],
}

impl TerminalRenderer {
    pub async fn new(window: Arc<Window>, config: RendererConfig) -> Option<Self> {
        let backend: Box<dyn TextBackend> = Box::new(CosmicTextBackend::new(BackendConfig {
            family: &config.font_family,
            font_size: config.font_size,
            scale: config.scale,
            adjust_cell_height: config.adjust_cell_height.as_deref(),
            adjust_cell_width: config.adjust_cell_width.as_deref(),
            features: &config.font_features,
            fallback: &config.font_fallback,
        }));
        let m = backend.metrics();
        let cell_size = [m.cell_width, m.cell_height];
        let gpu = GpuState::new(window).await;

        Some(Self {
            backend,
            cell_builder: CellBuilder::new(),
            gpu,
            theme: config.theme,
            min_contrast: config.min_contrast.clamp(1.0, 21.0),
            background_opacity: config.background_opacity.clamp(0.0, 1.0),
            cell_size,
            surface_width: config.width,
            surface_height: config.height,
            window_padding: config.window_padding,
        })
    }

    pub fn cell_size(&self) -> [f32; 2] {
        self.cell_size
    }

    pub fn grid_size(&self) -> (u16, u16) {
        let [cw, ch] = self.cell_size;
        let usable_w =
            (self.surface_width as f32 - 2.0 * f32::from(self.window_padding[0])).max(cw);
        let usable_h =
            (self.surface_height as f32 - 2.0 * f32::from(self.window_padding[1])).max(ch);
        let cols = (usable_w / cw) as u16;
        let rows = (usable_h / ch) as u16;
        (cols.max(1), rows.max(1))
    }

    pub fn pixel_to_grid(&self, x: f64, y: f64) -> (u16, u16) {
        let pad = self
            .cell_builder
            .last_frame()
            .map_or([0.0; 4], |fi| fi.grid_padding);
        let col = ((x as f32 - pad[0]) / self.cell_size[0]).max(0.0) as u16;
        let row = ((y as f32 - pad[1]) / self.cell_size[1]).max(0.0) as u16;
        (col, row)
    }

    pub fn resize_surface(&mut self, width: u32, height: u32) {
        self.surface_width = width;
        self.surface_height = height;
        self.gpu
            .resize(winit::dpi::PhysicalSize::new(width, height));
    }

    pub fn update_frame(&mut self, source: &mut dyn FrameSource) {
        let bg_color = self.effective_bg_color();
        let min_contrast = self.min_contrast;
        self.cell_builder.build_frame(
            source,
            self.backend.as_mut(),
            BuildFrameConfig {
                surface_width: self.surface_width,
                surface_height: self.surface_height,
                window_padding: self.window_padding,
                theme: &self.theme,
                bg_color,
                min_contrast,
            },
        );
        if let Some(fi) = self.cell_builder.last_frame() {
            self.gpu.update_image_frame(source, fi);
        }
    }

    pub fn render(&mut self, inputs: &RenderInputs) -> bool {
        let Some(fi) = self.cell_builder.last_frame() else {
            log::warn!("render: no frame built yet");
            return false;
        };
        self.gpu.render_frame(
            fi,
            CellFrame {
                bg_cells: self.cell_builder.bg_cells(),
                text_cells: self.cell_builder.text_cells(),
                dirty: self.cell_builder.last_dirty(),
            },
            self.cell_builder.atlas(),
            inputs,
            &self.theme,
        )
    }

    pub fn set_font_size(&mut self, points: f32) {
        self.backend.set_font_size(points);
        self.cell_builder.reset_glyphs();
        let m = self.backend.metrics();
        self.cell_size = [m.cell_width, m.cell_height];
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.backend.set_scale(scale);
        self.cell_builder.reset_glyphs();
        let m = self.backend.metrics();
        self.cell_size = [m.cell_width, m.cell_height];
    }

    pub fn set_adjust_cell_height(&mut self, value: Option<&str>) {
        self.backend.set_adjust_cell_height(value);
        let m = self.backend.metrics();
        self.cell_size = [m.cell_width, m.cell_height];
    }

    pub fn set_adjust_cell_width(&mut self, value: Option<&str>) {
        self.backend.set_adjust_cell_width(value);
        let m = self.backend.metrics();
        self.cell_size = [m.cell_width, m.cell_height];
    }

    /// Replace the active OpenType feature list. The renderer drops its
    /// shape and glyph caches because the same `(text, attrs)` key may
    /// resolve to different glyphs under the new feature set.
    pub fn set_font_features(&mut self, features: &[String]) {
        self.backend.set_features(features);
        self.cell_builder.reset_glyphs();
    }

    /// Replace the fallback family list. Drops shape and glyph caches so
    /// the next miss reconsiders the new fallback order.
    pub fn set_font_fallback(&mut self, fallback: &[String]) {
        self.backend.set_fallback(fallback);
        self.cell_builder.reset_glyphs();
    }

    /// Swap the theme. The theme is consumed CPU-side during the next
    /// `update_frame()` / `render()`, so no cache or GPU buffer needs to be
    /// touched — the caller just needs to trigger a repaint.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    pub fn set_min_contrast(&mut self, min_contrast: f32) {
        self.min_contrast = min_contrast.clamp(1.0, 21.0);
    }

    pub fn set_background_opacity(&mut self, opacity: f32) {
        self.background_opacity = opacity.clamp(0.0, 1.0);
    }

    /// Update the configured window padding. `grid_size()` shrinks
    /// accordingly, so callers should call `reflow()` afterwards to push the
    /// new cols/rows to the PTY.
    pub fn set_window_padding(&mut self, padding: [u16; 2]) {
        self.window_padding = padding;
    }

    fn effective_bg_color(&self) -> [u8; 4] {
        effective_bg_color(self.theme.bg, self.background_opacity)
    }
}

fn effective_bg_color(bg: [u8; 4], opacity: f32) -> [u8; 4] {
    let mut bg = bg;
    bg[3] = ((bg[3] as f32) * opacity.clamp(0.0, 1.0)).round() as u8;
    bg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_opacity_scales_theme_alpha() {
        assert_eq!(
            effective_bg_color([10, 20, 30, 200], 0.5),
            [10, 20, 30, 100]
        );
    }
}
