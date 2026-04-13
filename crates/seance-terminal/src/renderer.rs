use std::cell::Cell;
use std::sync::Arc;

use winit::window::Window;

use crate::Terminal;
use crate::font::{CellBuilder, GlyphCache};
use crate::gpu::GpuState;
pub use crate::gpu::uniforms::CursorShape;
use crate::selection::GridPos;
use crate::theme::Theme;

pub struct RendererConfig {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
}

pub struct Overlay {
    pub vt_cursor_visible: bool,
    pub cursor_shape: CursorShape,
    pub cursor_pos: GridPos,
    pub cursor_color: [f32; 4],
    pub selection: Option<(GridPos, GridPos)>,
    pub selection_color: [f32; 4],
}

impl Default for Overlay {
    fn default() -> Self {
        Self {
            vt_cursor_visible: true,
            cursor_shape: CursorShape::Hidden,
            cursor_pos: GridPos { col: 0, row: 0 },
            cursor_color: [1.0, 1.0, 1.0, 1.0],
            selection: None,
            selection_color: [0.3, 0.5, 0.8, 0.4],
        }
    }
}

pub struct TerminalRenderer {
    glyph_cache: GlyphCache,
    cell_builder: CellBuilder,
    gpu: GpuState,
    theme: Theme,
    cell_size: [f32; 2],
    grid_padding: Cell<[f32; 4]>,
    surface_width: u32,
    surface_height: u32,
    overlay: Overlay,
}

impl TerminalRenderer {
    pub async fn new(window: Arc<Window>, config: RendererConfig) -> Option<Self> {
        let glyph_cache = GlyphCache::new("JetBrainsMono Nerd Font", 14.0, config.scale);
        let metrics = glyph_cache.font.metrics();
        let cell_size = [metrics.cell_width, metrics.cell_height];

        let gpu = GpuState::new(window).await;

        Some(Self {
            glyph_cache,
            cell_builder: CellBuilder::new(),
            gpu,
            theme: Theme::default(),
            cell_size,
            grid_padding: Cell::new([0.0; 4]),
            surface_width: config.width,
            surface_height: config.height,
            overlay: Overlay::default(),
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
        let pad = self.grid_padding.get();
        let col = ((x as f32 - pad[0]) / self.cell_size[0]).max(0.0) as u16;
        let row = ((y as f32 - pad[1]) / self.cell_size[1]).max(0.0) as u16;
        (col, row)
    }

    pub fn resize_surface(&mut self, width: u32, height: u32, _scale: f64) {
        self.surface_width = width;
        self.surface_height = height;
        self.gpu
            .resize(winit::dpi::PhysicalSize::new(width, height));
    }

    pub fn overlay_mut(&mut self) -> &mut Overlay {
        &mut self.overlay
    }

    pub fn update_frame(&mut self, terminal: &mut Terminal) {
        let ok = self.cell_builder.build_frame(
            terminal.vt_mut(),
            &mut self.glyph_cache,
            self.surface_width,
            self.surface_height,
            &self.theme,
        );
        if ok
            && let Some(fi) = self.cell_builder.last_frame()
        {
            self.grid_padding.set(fi.grid_padding);
        }
    }

    pub fn render(&mut self) -> bool {
        let Some(fi) = self.cell_builder.last_frame() else {
            return false;
        };
        self.gpu.render_frame(
            fi,
            self.cell_builder.bg_cells(),
            self.cell_builder.text_cells(),
            &self.glyph_cache,
            &self.overlay,
        )
    }

    pub fn set_font_size(&mut self, points: f32) {
        self.glyph_cache.set_font_size(points);
        let metrics = self.glyph_cache.font.metrics();
        self.cell_size = [metrics.cell_width, metrics.cell_height];
    }
}
