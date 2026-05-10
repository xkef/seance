use std::sync::Arc;

use seance_config::Theme;
use seance_frame::FrameSource;
use seance_protocol::{GridPos, ImageCacheEvent, MouseSize};
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

    /// Pixel-space geometry the libghostty-vt mouse encoder needs to
    /// translate surface-space cursor positions into VT cell coordinates.
    /// `grid_padding` layout matches `pixel_to_grid` above:
    /// `[left, top, right, bottom]`.
    pub fn mouse_size(&self) -> MouseSize {
        let pad = self
            .cell_builder
            .last_frame()
            .map_or([0.0; 4], |fi| fi.grid_padding);
        MouseSize {
            screen_width: self.surface_width,
            screen_height: self.surface_height,
            cell_width: self.cell_size[0].max(1.0) as u32,
            cell_height: self.cell_size[1].max(1.0) as u32,
            padding_left: pad[0].max(0.0) as u32,
            padding_top: pad[1].max(0.0) as u32,
            padding_right: pad[2].max(0.0) as u32,
            padding_bottom: pad[3].max(0.0) as u32,
        }
    }

