//! Terminal renderer: manages the ghostty renderer and GPU pipeline.
//!
//! The renderer is responsible for:
//! - Font loading and glyph atlas management (via libghostty)
//! - Converting terminal state to GPU-ready cell buffers
//! - Presenting frames to the window surface via wgpu
//!
//! # Multiplexing
//!
//! For split-pane rendering, the typical flow is:
//!
//! 1. Attach a terminal to the renderer
//! 2. Call [`update_frame`](TerminalRenderer::update_frame) to rebuild
//!    cell buffers from the terminal state
//! 3. Call [`render`](TerminalRenderer::render) or
//!    [`render_bg_only`](TerminalRenderer::render_bg_only) to present
//!
//! In a multiplexed setup, the app iterates panes and calls `attach` +
//! `update_frame` for each, then renders them into viewports. Currently
//! we render a single pane per frame; multi-viewport compositing will
//! extend this in Phase 2.

use std::ffi::CString;
use std::sync::Arc;

use winit::window::Window;

use ghostty_renderer as gr;

use crate::gpu::GpuState;
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
}

impl TerminalRenderer {
    /// Create a new renderer for the given window.
    ///
    /// This initializes the ghostty renderer (font loading, glyph atlas)
    /// and the wgpu GPU pipeline. The renderer is ready to render after
    /// this call.
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
        })
    }

    /// Cell dimensions in pixels `[width, height]`.
    ///
    /// Determined by the loaded font; constant for the renderer's lifetime.
    pub fn cell_size(&self) -> [f32; 2] {
        self.cell_size
    }

    /// Notify the renderer of a surface resize.
    pub fn resize_surface(&mut self, width: u32, height: u32, scale: f64) {
        self.renderer.resize(width, height, scale);
        self.gpu.resize(winit::dpi::PhysicalSize::new(width, height));
    }

    // -- Terminal attachment --

    /// Attach a terminal to the renderer for the next frame.
    ///
    /// This sets the ghostty renderer's terminal reference so that
    /// `update_frame` reads from this terminal's state.
    pub fn attach(&self, terminal: &Terminal) {
        self.renderer.set_terminal(terminal.vt());
    }

    /// Rebuild cell buffers from the currently attached terminal.
    ///
    /// Call this after `attach` and before `render`.
    pub fn update_frame(&self) {
        self.renderer.update_frame();
    }

    // -- Rendering --

    /// Render a full frame (background + cell backgrounds + text).
    ///
    /// Call `attach` + `update_frame` first.
    pub fn render(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame(&snapshot)
    }

    /// Render only the background color (no cell content).
    ///
    /// Used during resize transitions to avoid showing stale content
    /// while the shell processes SIGWINCH.
    pub fn render_bg_only(&mut self) -> bool {
        let snapshot = self.renderer.frame_snapshot();
        self.gpu.render_frame_bg_only(&snapshot)
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
    ///
    /// A value of 1.0 disables contrast adjustment. Values like 4.5
    /// (WCAG AA) or 7.0 (WCAG AAA) enforce readability.
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
