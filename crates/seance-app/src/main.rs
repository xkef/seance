mod terminal;

use std::collections::HashMap;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use ghostty_renderer::{Blending, Color, Renderer, RendererConfig, Terminal};
use seance_gpu::GpuState;
use seance_input::{Action, InputHandler};
use seance_layout::{LayoutTree, PaneId};

use crate::terminal::TerminalView;

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    // Window + GPU
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    gpu: Option<GpuState>,

    // Panes
    panes: HashMap<PaneId, TerminalView>,
    layout: LayoutTree,
    focused: PaneId,

    // Input
    input: InputHandler,
    modifiers: Modifiers,

    // Metrics (constant after init)
    cell_size: [f32; 2],

    // Frame scheduling
    /// True when cell buffers need to be rebuilt from terminal state.
    content_dirty: bool,
    /// Frames to wait after PTY data before committing a render update.
    /// Batches rapid bursts of output into a single `update_frame`.
    cooldown: u8,
    /// Suppress `update_frame` after a resize until the shell redraws.
    /// While set, we render only the background color — no stale content.
    resize_pending: bool,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            gpu: None,
            panes: HashMap::new(),
            layout: LayoutTree::new(0, 1.0, 1.0),
            focused: 0,
            input: InputHandler::new(),
            modifiers: Modifiers::default(),
            cell_size: [0.0, 0.0],
            content_dirty: true,
            cooldown: 0,
            resize_pending: false,
        }
    }

    // -- helpers -------------------------------------------------------------

    fn grid_size_for_pixels(&self, width: u32, height: u32) -> (u16, u16) {
        let [cw, ch] = self.cell_size;
        if cw <= 0.0 || ch <= 0.0 {
            return (80, 24);
        }
        let cols = (width as f32 / cw).floor().max(1.0) as u16;
        let rows = (height as f32 / ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    // -- I/O -----------------------------------------------------------------

    /// Poll all panes for PTY output. Returns true if any data arrived.
    fn poll_pty(&mut self) -> bool {
        let mut got_data = false;
        for view in self.panes.values_mut() {
            got_data |= view.poll();
        }
        got_data
    }

    // -- frame loop ----------------------------------------------------------

    fn draw(&mut self) {
        let got_data = self.poll_pty();

        // --- frame scheduling ---
        if got_data {
            self.content_dirty = true;
            self.cooldown = 2; // wait 2 frames to batch rapid output

            // Shell has started redrawing after resize — unfreeze.
            if self.resize_pending {
                self.resize_pending = false;
            }
        } else if self.cooldown > 0 {
            self.cooldown -= 1;
        } else if self.content_dirty {
            // Cooldown elapsed, commit the update.
            self.content_dirty = false;
            if let Some(renderer) = &self.renderer {
                renderer.update_frame();
            }
        }

        // --- render ---
        let Some(renderer) = &self.renderer else { return };
        let Some(gpu) = &mut self.gpu else { return };

        if self.resize_pending {
            // Render background-only frame: avoids showing a half-drawn
            // terminal while the shell is processing SIGWINCH.
            let snapshot = renderer.frame_snapshot();
            gpu.render_frame_bg_only(&snapshot);
        } else {
            let snapshot = renderer.frame_snapshot();
            gpu.render_frame(&snapshot);
        }
    }
}

// ---------------------------------------------------------------------------
// winit event handler
// ---------------------------------------------------------------------------

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes().with_title("séance");
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        let size = window.inner_size();
        let scale = window.scale_factor();
        let native_handle = get_native_handle(&window);

        let config = RendererConfig {
            width_px: size.width,
            height_px: size.height,
            content_scale: scale,
            alpha_blending: Blending::Linear,
            ..Default::default()
        };
        let renderer =
            Renderer::new(&config, native_handle).expect("failed to create renderer");

        remove_ghostty_layer(&window);

        let gpu = pollster::block_on(GpuState::new(window.clone()));

        self.renderer = Some(renderer);
        self.gpu = Some(gpu);
        self.window = Some(window);

        // Read cell metrics from a throwaway terminal. Cell size is
        // determined by font metrics and stays constant.
        if let Some(r) = &self.renderer {
            let tmp = Terminal::new(80, 24).expect("temp terminal");
            r.set_terminal(&tmp);
            r.update_frame();
            let snap = r.frame_snapshot();
            let fd = snap.frame_data();
            self.cell_size = [fd.cell_width, fd.cell_height];
            self.layout.set_cell_size(fd.cell_width, fd.cell_height);
            drop(snap);
        }

        let (cols, rows) = self.grid_size_for_pixels(size.width, size.height);
        let view = TerminalView::spawn(cols, rows);

        if let Some(r) = &self.renderer {
            r.set_terminal(view.terminal());
            r.set_background(Color { r: 0x1e, g: 0x1e, b: 0x2e });
        }

        self.panes.insert(0, view);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Resized(new_size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(new_size);
                }
                if let (Some(r), Some(w)) = (&self.renderer, &self.window) {
                    r.resize(new_size.width, new_size.height, w.scale_factor());
                }
                let (cols, rows) = self.grid_size_for_pixels(new_size.width, new_size.height);
                for view in self.panes.values_mut() {
                    view.resize(cols, rows);
                }

                // Freeze rendering until the shell redraws after SIGWINCH.
                // We'll show only the background color in the meantime.
                self.resize_pending = true;

                with_metal_layer(&self.window, |layer| {
                    set_presents_with_transaction(layer, true);
                });
                self.draw();
                with_metal_layer(&self.window, |layer| {
                    set_presents_with_transaction(layer, false);
                });
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let modes = self.panes
                    .get(&self.focused)
                    .map_or(Default::default(), |v| v.modes());
                let action = self.input.handle_key(&event, &self.modifiers, modes);
                match action {
                    Action::WritePty(data) => {
                        if let Some(view) = self.panes.get(&self.focused) {
                            view.write(&data);
                        }
                    }
                    Action::Ignore => {}
                    _ => {
                        log::debug!("unhandled action: {action:?}");
                    }
                }
                self.content_dirty = true;
                self.request_redraw();
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => {
                        let ch = self.cell_size[1].max(1.0);
                        (pos.y / ch as f64) as i32
                    }
                };
                if lines != 0 {
                    let modes = self.panes
                        .get(&self.focused)
                        .map_or(Default::default(), |v| v.modes());
                    if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
                        if let Some(view) = self.panes.get(&self.focused) {
                            view.write(&data);
                        }
                    } else if let Some(view) = self.panes.get_mut(&self.focused) {
                        view.scroll(ghostty_renderer::ScrollAction::Lines(-lines));
                    }
                    self.content_dirty = true;
                    self.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                self.draw();
                self.request_redraw();
            }

            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn remove_ghostty_layer(window: &Window) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;

    let nsview = get_native_handle(window);
    unsafe {
        let view: *mut AnyObject = nsview.cast();
        let _: () = msg_send![view, setWantsLayer: false];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![view, setLayer: nil];
    }
}

#[cfg(not(target_os = "macos"))]
fn remove_ghostty_layer(_window: &Window) {}

#[cfg(target_os = "macos")]
fn with_metal_layer(window: &Option<Arc<Window>>, f: impl FnOnce(*mut objc2::runtime::AnyObject)) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    let Some(window) = window else { return };
    let nsview = get_native_handle(window);
    unsafe {
        let view: *mut AnyObject = nsview.cast();
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() {
            return;
        }
        let sublayers: *mut AnyObject = msg_send![layer, sublayers];
        if sublayers.is_null() {
            return;
        }
        let count: usize = msg_send![sublayers, count];
        let Some(metal_class) = AnyClass::get("CAMetalLayer") else {
            return;
        };
        for i in 0..count {
            let sublayer: *mut AnyObject = msg_send![sublayers, objectAtIndex: i];
            let is_metal: bool = msg_send![sublayer, isKindOfClass: metal_class];
            if is_metal {
                f(sublayer);
                return;
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn with_metal_layer(_window: &Option<Arc<Window>>, _f: impl FnOnce(*mut u8)) {}

#[cfg(target_os = "macos")]
fn set_presents_with_transaction(layer: *mut objc2::runtime::AnyObject, value: bool) {
    use objc2::msg_send;
    unsafe {
        let _: () = msg_send![layer, setPresentsWithTransaction: value];
    }
}

#[cfg(target_os = "macos")]
fn get_native_handle(window: &Window) -> *mut std::ffi::c_void {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = window.window_handle().expect("no window handle");
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr(),
        _ => panic!("expected AppKit window handle"),
    }
}

#[cfg(not(target_os = "macos"))]
fn get_native_handle(_window: &Window) -> *mut std::ffi::c_void {
    // Level 2 (consumer-draws): no native surface handle needed.
    // The renderer produces cell buffers; we own the GPU pipeline via wgpu.
    std::ptr::null_mut()
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
