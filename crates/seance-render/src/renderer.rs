use std::sync::Arc;

use seance_vt::{FrameSource, GridPos};
use winit::window::Window;

use crate::gpu::GpuState;
pub use crate::gpu::uniforms::CursorShape;
use crate::text::CellBuilder;
use crate::text::backend::TextBackend;
use crate::text::cosmic::CosmicTextBackend;
use crate::theme::Theme;

pub struct RendererConfig {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub font_family: String,
    pub font_size: f32,
}

/// Per-frame dynamic state the app supplies to the renderer.
#[derive(Debug, Clone)]
pub struct RenderInputs {
    pub vt_cursor_visible: bool,
    pub cursor_shape: CursorShape,
    pub cursor_pos: GridPos,
    pub selection: Option<(GridPos, GridPos)>,
}

impl Default for RenderInputs {
    fn default() -> Self {
        Self {
            vt_cursor_visible: true,
            cursor_shape: CursorShape::Hidden,
            cursor_pos: GridPos { col: 0, row: 0 },
            selection: None,
        }
    }
}

pub struct TerminalRenderer {
    backend: Box<dyn TextBackend>,
    cell_builder: CellBuilder,
    gpu: GpuState,
    theme: Theme,
    cell_size: [f32; 2],
    surface_width: u32,
    surface_height: u32,
}

impl TerminalRenderer {
    pub async fn new(window: Arc<Window>, config: RendererConfig) -> Option<Self> {
        let backend: Box<dyn TextBackend> = Box::new(CosmicTextBackend::new(
            &config.font_family,
            config.font_size,
            config.scale,
        ));
        let m = backend.metrics();
        let cell_size = [m.cell_width, m.cell_height];
        let gpu = GpuState::new(window).await;

        Some(Self {
            backend,
            cell_builder: CellBuilder::new(),
            gpu,
            theme: Theme::default(),
            cell_size,
            surface_width: config.width,
            surface_height: config.height,
        })
    }

    pub fn cell_size(&self) -> [f32; 2] {
        self.cell_size
    }

    pub fn grid_size(&self) -> (u16, u16) {
        let [cw, ch] = self.cell_size;
        let cols = (self.surface_width as f32 / cw) as u16;
        let rows = (self.surface_height as f32 / ch) as u16;
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
        self.cell_builder.build_frame(
            source,
            self.backend.as_mut(),
            self.surface_width,
            self.surface_height,
            &self.theme,
        );
    }

    pub fn render(&mut self, inputs: &RenderInputs) -> bool {
        let Some(fi) = self.cell_builder.last_frame() else {
            log::warn!("render: no frame built yet");
            return false;
        };
        self.gpu.render_frame(
            fi,
            self.cell_builder.bg_cells(),
            self.cell_builder.text_cells(),
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
}
