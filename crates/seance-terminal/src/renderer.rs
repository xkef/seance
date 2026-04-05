//! Terminal renderer: manages the ghostty renderer and GPU pipeline.
//!
//! The renderer is responsible for:
//! - Font loading and glyph atlas management (via libghostty)
//! - Converting terminal state to GPU-ready cell buffers
//! - Presenting frames to the window surface via wgpu
//!
//! # Overlay
//!
//! The renderer supports an overlay layer for multiplexer UI:
//! - A cursor with configurable shape (block/underline/bar)
//! - Selection highlighting over a range of cells
//! - The ability to hide the VT cursor (so it doesn't fight the overlay)
//!
//! Set the overlay state before calling [`render`](TerminalRenderer::render).

use std::ffi::CString;
use std::sync::Arc;

use winit::window::Window;

use ghostty_renderer as gr;

use crate::gpu::GpuState;
pub use crate::gpu::uniforms::CursorShape;
use crate::selection::GridPos;
use crate::Terminal;

/// Configuration for creating a [`TerminalRenderer`].
pub struct RendererConfig {
    /// Initial surface width in pixels.
    pub width: u32,
    /// Initial surface height in pixels.
    pub height: u32,
    /// Display scale factor (e.g. 2.0 for Retina).
    pub scale: f64,
    /// Native window handle (NSView* on macOS, null for Level 2).
    pub native_handle: *mut std::ffi::c_void,
}

/// Overlay state for the current frame.
///
/// Controls the multiplexer's visual overlay: cursor, selection highlight,
/// and whether the VT (terminal) cursor is visible.
///
/// # Example: copy mode
///
/// ```no_run
/// # use seance_terminal::*;
/// # use seance_terminal::renderer::CursorShape;
/// # let renderer: TerminalRenderer = todo!();
/// let overlay = renderer.overlay_mut();
/// overlay.vt_cursor_visible = false;                     // hide VT cursor
/// overlay.cursor_shape = CursorShape::Block;             // show block cursor
/// overlay.cursor_pos = GridPos { col: 5, row: 10 };      // at copy position
/// overlay.cursor_color = [0.8, 0.8, 0.2, 1.0];          // yellow
/// overlay.selection = Some((                              // highlight selection
///     GridPos { col: 0, row: 8 },
///     GridPos { col: 20, row: 10 },
/// ));
/// ```
pub struct Overlay {
    /// Whether the VT (terminal emulator) cursor should be rendered.
    /// Set to `false` in copy mode to prevent it from fighting with
    /// the overlay cursor.
    pub vt_cursor_visible: bool,

    /// Shape of the overlay cursor. `CursorShape::Hidden` disables it.
    pub cursor_shape: CursorShape,

    /// Grid position of the overlay cursor.
    pub cursor_pos: GridPos,

    /// Overlay cursor color as linear RGBA floats.
    /// Default: white fully opaque.
    pub cursor_color: [f32; 4],

    /// Selection range as `(start, end)` in normalized reading order
    /// (start <= end). `None` means no selection highlight.
    pub selection: Option<(GridPos, GridPos)>,

    /// Selection highlight color as sRGB RGBA floats.
    /// Default: semi-transparent blue.
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
/// Wraps the ghostty renderer (font shaping, cell buffer generation)
/// and the wgpu pipeline (GPU upload and presentation). The consumer
/// interacts with [`Terminal`] objects and this renderer handles all
/// the rendering details.
pub struct TerminalRenderer {
    renderer: gr::Renderer,
    gpu: GpuState,
    cell_size: [f32; 2],
    overlay: Overlay,
}

impl TerminalRenderer {
    /// Create a new renderer for the given window.
    pub async fn new(window: Arc<Window>, config: RendererConfig) -> Option<Self> {
        let renderer_config = gr::RendererConfig {
            width_px: config.width,
            height_px: config.height,
            content_scale: config.scale,
            alpha_blending: gr::Blending::Linear,
            ..Default::default()
        };

        let renderer = gr::Renderer::new(&renderer_config, config.native_handle)?;
        remove_ghostty_layer(&window);

        let gpu = GpuState::new(window).await;

        // Probe cell metrics from a throwaway terminal.
        let tmp = gr::Terminal::new(80, 24)?;
        renderer.set_terminal(&tmp);
        renderer.update_frame();
        let snap = renderer.frame_snapshot();
        let fd = snap.frame_data();
        let cell_size = [fd.cell_width, fd.cell_height];
        drop(snap);

        Some(Self {
            renderer,
            gpu,
            cell_size,
            overlay: Overlay::default(),
        })
    }

    /// Cell dimensions in pixels `[width, height]`.
    pub fn cell_size(&self) -> [f32; 2] {
        self.cell_size
    }

    /// Notify the renderer of a surface resize.
    pub fn resize_surface(&mut self, width: u32, height: u32, scale: f64) {
        self.renderer.resize(width, height, scale);
        self.gpu.resize(winit::dpi::PhysicalSize::new(width, height));
    }

    // -- Overlay --

    /// Mutable access to the overlay state.
    ///
    /// Modify this before calling [`render`] to control the overlay
    /// cursor, selection highlight, and VT cursor visibility.
    pub fn overlay_mut(&mut self) -> &mut Overlay {
        &mut self.overlay
    }

    /// Read-only access to the overlay state.
    pub fn overlay(&self) -> &Overlay {
        &self.overlay
    }

    // -- Terminal attachment --

    /// Attach a terminal to the renderer for the next frame.
    pub fn attach(&self, terminal: &Terminal) {
        self.renderer.set_terminal(terminal.vt());
    }

    /// Rebuild cell buffers from the currently attached terminal.
    pub fn update_frame(&self) {
        self.renderer.update_frame();
    }

    // -- Rendering --

    /// Render a full frame (background + cell backgrounds + selection +
    /// overlay cursor + text).
    pub fn render(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame(&snapshot, &self.overlay)
    }

    /// Render only the background color (no cell content).
    pub fn render_bg_only(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame_bg_only(&snapshot, &self.overlay)
    }

    // -- Appearance --

    /// Load a named theme (searches Ghostty's built-in theme list).
    pub fn set_theme(&self, name: &str) -> bool {
        let Ok(cname) = CString::new(name) else {
            return false;
        };
        self.renderer.load_theme(&cname)
    }

    /// Load a theme from a file path.
    pub fn set_theme_file(&self, path: &str) -> bool {
        let Ok(cpath) = CString::new(path) else {
            return false;
        };
        self.renderer.load_theme_file(&cpath)
    }

    /// Set the font size in points.
    pub fn set_font_size(&self, points: f32) {
        self.renderer.set_font_size(points);
    }

    /// Set the background color.
    pub fn set_background(&self, color: gr::Color) {
        self.renderer.set_background(color);
    }

    /// Set the foreground (text) color.
    pub fn set_foreground(&self, color: gr::Color) {
        self.renderer.set_foreground(color);
    }

    /// Set background opacity (0.0 = transparent, 1.0 = opaque).
    pub fn set_background_opacity(&self, opacity: f32) {
        self.renderer.set_background_opacity(opacity);
    }

    /// Set minimum contrast ratio for text vs background (WCAG).
    pub fn set_min_contrast(&self, contrast: f32) {
        self.renderer.set_min_contrast(contrast);
    }

    /// Set the 256-color palette.
    pub fn set_palette(&self, palette: &[gr::Color; 256]) {
        self.renderer.set_palette(palette);
    }
}

// ---------------------------------------------------------------------------
// Platform helpers (internal)
// ---------------------------------------------------------------------------

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
