//! Terminal renderer: ghostty font grid + wgpu pipeline.
//!
//! Bridges the ghostty-renderer cell buffer generator with the wgpu GPU
//! pipeline. Each frame, `update_frame` asks ghostty to rasterise the
//! visible terminal content into cell arrays, then `render` uploads those
//! arrays and draws three passes (background, cell bg, cell text).

use std::cell::Cell;
use std::ffi::CString;
use std::rc::Rc;
use std::sync::Arc;

use winit::window::Window;

use ghostty_renderer as gr;

use crate::Terminal;
use crate::gpu::GpuState;
pub use crate::gpu::uniforms::CursorShape;
use crate::selection::GridPos;

/// Initial configuration for creating a [`TerminalRenderer`].
pub struct RendererConfig {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub native_handle: *mut std::ffi::c_void,
}

/// Per-frame overlay state drawn on top of the terminal content:
/// cursor shape/position and text selection highlight.
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

/// GPU-accelerated terminal renderer.
///
/// Owns the ghostty font grid, cell buffer renderer, and wgpu state.
/// Not `Send` — must stay on the main thread.
pub struct TerminalRenderer {
    font_grid: Rc<gr::FontGrid>,
    renderer: gr::Renderer,
    gpu: GpuState,
    cell_size: [f32; 2],
    grid_padding: Cell<[f32; 4]>,
    surface_width: u32,
    surface_height: u32,
    overlay: Overlay,
}

impl TerminalRenderer {
    /// Create a new renderer for the given window.
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

        let font_grid = Rc::new(gr::FontGrid::new(&font_grid_config)?);
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
            grid_padding: Cell::new([0.0; 4]),
            overlay: Overlay::default(),
        })
    }

    /// Cell dimensions in pixels: `[width, height]`.
    pub fn cell_size(&self) -> [f32; 2] {
        self.cell_size
    }

    /// Compute the terminal grid size (columns, rows) from the current
    /// surface dimensions, subtracting padding.
    pub fn grid_size(&self) -> (u16, u16) {
        let [cw, ch] = self.cell_size;
        let pad = self.grid_padding.get();
        let usable_w = (self.surface_width as f32 - pad[0] - pad[2]).max(cw);
        let usable_h = (self.surface_height as f32 - pad[1] - pad[3]).max(ch);
        let cols = (usable_w / cw).max(1.0) as u16;
        let rows = (usable_h / ch).max(1.0) as u16;
        (cols, rows)
    }

    /// Convert a pixel position to a grid cell (col, row).
    pub fn pixel_to_grid(&self, x: f64, y: f64) -> (u16, u16) {
        let pad = self.grid_padding.get();
        let col = ((x as f32 - pad[0]) / self.cell_size[0]).max(0.0) as u16;
        let row = ((y as f32 - pad[1]) / self.cell_size[1]).max(0.0) as u16;
        (col, row)
    }

    /// Notify the renderer and GPU of a new surface size.
    pub fn resize_surface(&mut self, width: u32, height: u32, _scale: f64) {
        self.surface_width = width;
        self.surface_height = height;
        self.renderer.resize(width, height);
        self.gpu
            .resize(winit::dpi::PhysicalSize::new(width, height));
    }

    /// Mutable access to the overlay state (cursor, selection).
    pub fn overlay_mut(&mut self) -> &mut Overlay {
        &mut self.overlay
    }

    /// Bind a terminal to this renderer so its content is drawn.
    pub fn attach(&self, terminal: &Terminal) {
        // Safety: raw_terminal_ptr() returns a valid GhosttyTerminal handle.
        unsafe { self.renderer.set_terminal_raw(terminal.raw_terminal_ptr()) };
    }

    /// Rebuild the cell buffer from the current terminal state.
    /// Also caches the grid padding for `grid_size` calculations.
    pub fn update_frame(&self) {
        self.renderer.update_frame(true);
        let fd = self.renderer.frame_snapshot().frame_data();
        self.grid_padding.set(fd.grid_padding);
    }

    /// Upload cell data and render one frame to the surface.
    pub fn render(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame(&snapshot, &self.overlay)
    }

    /// Load a named theme from the ghostty resources directory.
    pub fn set_theme(&self, name: &str) -> bool {
        CString::new(name)
            .ok()
            .is_some_and(|c| self.renderer.load_theme(&c))
    }

    /// Change the font size and update cached cell metrics.
    pub fn set_font_size(&mut self, points: f32) {
        self.font_grid.set_size(points);
        let metrics = self.font_grid.metrics();
        self.cell_size = [metrics.cell_width, metrics.cell_height];
    }
}

/// Remove the CAMetalLayer that ghostty's renderer installs on the NSView.
/// We use our own wgpu surface instead.
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
