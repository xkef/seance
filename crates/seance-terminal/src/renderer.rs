//! Terminal renderer: ghostty font grid + renderer + wgpu pipeline.

use std::ffi::CString;
use std::sync::Arc;

use winit::window::Window;

use ghostty_renderer as gr;

use crate::gpu::GpuState;
pub use crate::gpu::uniforms::CursorShape;
use crate::selection::GridPos;
use crate::Terminal;

pub struct RendererConfig {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub native_handle: *mut std::ffi::c_void,
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
    font_grid: Arc<gr::FontGrid>,
    renderer: gr::Renderer,
    gpu: GpuState,
    cell_size: [f32; 2],
    surface_width: u32,
    surface_height: u32,
    overlay: Overlay,
}

impl TerminalRenderer {
    pub async fn new(window: Arc<Window>, config: RendererConfig) -> Option<Self> {
        let font_family = CString::new("JetBrainsMono Nerd Font").unwrap();
        let font_feature = CString::new("calt").unwrap();

        let font_grid_config = gr::FontGridConfig {
            font_size: 14.0,
            font_family: font_family.as_ptr(),
            font_features: font_feature.as_ptr(),
            content_scale: config.scale,
            ..Default::default()
        };

        let font_grid = Arc::new(gr::FontGrid::new(&font_grid_config)?);
        let metrics = font_grid.metrics();
        let cell_size = [metrics.cell_width, metrics.cell_height];

        let renderer_config = gr::RendererConfig {
            width_px: config.width,
            height_px: config.height,
            content_scale: config.scale,
            native_view: config.native_handle,
            min_contrast: 1.1,
            background_opacity: 1.0,
            alpha_blending: gr::Blending::Native,
            ..Default::default()
        };

        let renderer = gr::Renderer::new(font_grid.clone(), &renderer_config)?;
        remove_ghostty_layer(&window);
        let gpu = GpuState::new(window).await;

        Some(Self {
            font_grid,
            renderer,
            surface_width: config.width,
            surface_height: config.height,
            gpu,
            cell_size,
            overlay: Overlay::default(),
        })
    }

    pub fn cell_size(&self) -> [f32; 2] { self.cell_size }

    pub fn grid_size(&self) -> (u16, u16) {
        let [cw, ch] = self.cell_size;
        let cols = (self.surface_width as f32 / cw).max(1.0) as u16;
        let rows = (self.surface_height as f32 / ch).max(1.0) as u16;
        (cols, rows)
    }

    pub fn font_grid(&self) -> &Arc<gr::FontGrid> { &self.font_grid }

    pub fn resize_surface(&mut self, width: u32, height: u32, _scale: f64) {
        self.surface_width = width;
        self.surface_height = height;
        self.renderer.resize(width, height);
        self.gpu.resize(winit::dpi::PhysicalSize::new(width, height));
    }

    pub fn resize_gpu(&mut self, width: u32, height: u32) {
        self.gpu.resize(winit::dpi::PhysicalSize::new(width, height));
    }

    pub fn resize_renderer(&mut self, width: u32, height: u32) {
        self.renderer.resize(width, height);
    }

    pub fn overlay_mut(&mut self) -> &mut Overlay { &mut self.overlay }
    pub fn overlay(&self) -> &Overlay { &self.overlay }

    pub fn attach(&self, terminal: &Terminal) {
        self.renderer.set_terminal_raw(terminal.raw_terminal_ptr());
    }

    pub fn update_frame(&self) {
        self.renderer.update_frame(true);
    }

    pub fn render(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame(&snapshot, &self.overlay)
    }

    pub fn render_bg_only(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame_bg_only(&snapshot, &self.overlay)
    }

    pub fn set_theme(&self, name: &str) -> bool {
        CString::new(name).ok().map_or(false, |c| self.renderer.load_theme(&c))
    }

    pub fn set_theme_file(&self, path: &str) -> bool {
        CString::new(path).ok().map_or(false, |c| self.renderer.load_theme_file(&c))
    }

    pub fn set_font_size(&self, points: f32) { self.font_grid.set_size(points); }
    pub fn set_background(&self, color: gr::Color) { self.renderer.set_background(color); }
    pub fn set_foreground(&self, color: gr::Color) { self.renderer.set_foreground(color); }
    pub fn set_background_opacity(&self, opacity: f32) { self.renderer.set_background_opacity(opacity); }
    pub fn set_min_contrast(&self, contrast: f32) { self.renderer.set_min_contrast(contrast); }
    pub fn set_palette(&self, palette: &[gr::Color; 256]) { self.renderer.set_palette(palette); }
}

#[cfg(target_os = "macos")]
fn remove_ghostty_layer(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle().expect("no window handle");
    let nsview = match handle.as_raw() {
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr(),
        _ => return,
    };
    unsafe {
        let view: *mut AnyObject = nsview.cast();
        let _: () = msg_send![view, setWantsLayer: false];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![view, setLayer: nil];
    }
}

#[cfg(not(target_os = "macos"))]
fn remove_ghostty_layer(_window: &Window) {}
