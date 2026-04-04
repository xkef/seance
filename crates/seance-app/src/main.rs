use std::collections::HashMap;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use ghostty_renderer::{Renderer, RendererConfig, Terminal};
use seance_gpu::GpuState;
use seance_input::{Action, InputHandler, TerminalModes};
use seance_layout::{LayoutTree, PaneId};
use seance_pty::Pty;

struct Pane {
    terminal: Terminal,
    pty: Pty,
}

struct App {
    renderer: Option<Renderer>,
    gpu: Option<GpuState>,
    window: Option<Arc<Window>>,
    panes: HashMap<PaneId, Pane>,
    layout: LayoutTree,
    focused: PaneId,
    input: InputHandler,
    modifiers: Modifiers,
    cell_size: [f32; 2],
    needs_redraw: bool,
    data_cooldown: u8,
    last_text_count: usize,
}

impl App {
    fn new() -> Self {
        Self {
            renderer: None,
            gpu: None,
            window: None,
            panes: HashMap::new(),
            layout: LayoutTree::new(0, 1.0, 1.0),
            focused: 0,
            input: InputHandler::new(),
            modifiers: Modifiers::default(),
            cell_size: [0.0, 0.0],
            needs_redraw: true,
            data_cooldown: 0,
            last_text_count: 0,
        }
    }

    fn focused_term_modes(&self) -> TerminalModes {
        self.panes.get(&self.focused).map_or(TerminalModes::default(), |p| {
            TerminalModes {
                cursor_keys: p.terminal.mode_cursor_keys(),
                mouse_event: p.terminal.mode_mouse_event(),
                mouse_format_sgr: p.terminal.mode_mouse_format_sgr(),
            }
        })
    }

    fn grid_size_for_pixels(&self, width: u32, height: u32) -> (u16, u16) {
        let cw = self.cell_size[0];
        let ch = self.cell_size[1];
        if cw <= 0.0 || ch <= 0.0 {
            return (80, 24);
        }
        let cols = (width as f32 / cw).floor().max(1.0) as u16;
        let rows = (height as f32 / ch).floor().max(1.0) as u16;
        (cols, rows)
    }

    fn spawn_pane(&mut self, id: PaneId, cols: u16, rows: u16) {
        let terminal = Terminal::new(cols, rows).expect("failed to create terminal");
        let pty = Pty::spawn(seance_pty::Size { cols, rows }).expect("failed to spawn PTY");
        self.panes.insert(id, Pane { terminal, pty });
    }

    fn resize_panes(&mut self, cols: u16, rows: u16) {
        for pane in self.panes.values_mut() {
            pane.terminal.resize(cols, rows);
            let _ = pane.pty.resize(seance_pty::Size { cols, rows });
        }
    }

    fn pump_pty_io(&mut self) -> bool {
        let mut got_data = false;
        let mut buf = [0u8; 4096];
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in &ids {
            let pane = self.panes.get_mut(id).unwrap();
            loop {
                match pane.pty.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        pane.terminal.vt_write(&buf[..n]);
                        got_data = true;
                    }
                    _ => break,
                }
            }
            let responses = pane.terminal.drain_responses();
            if !responses.is_empty() {
                let _ = pane.pty.write_all(responses);
            }
            pane.terminal.clear_responses();
        }
        got_data
    }

    fn draw(&mut self) {
        let got_data = self.pump_pty_io();

        if got_data {
            self.needs_redraw = true;
            self.data_cooldown = 2;
        } else if self.data_cooldown > 0 {
            self.data_cooldown -= 1;
        } else if self.needs_redraw {
            self.needs_redraw = false;
            if let Some(renderer) = &self.renderer {
                renderer.update_frame();

                let snap = renderer.frame_snapshot();
                let n = snap.text_cells().len();
                if self.last_text_count > 0 && n < self.last_text_count / 2 {
                    // Content regressed (e.g., terminal cleared after resize
                    // but shell hasn't redrawn yet). Keep old frame, wait
                    // for more data.
                    self.needs_redraw = true;
                    return;
                }
                self.last_text_count = n;
            }
        }

        if let (Some(renderer), Some(gpu)) = (&self.renderer, &mut self.gpu) {
            let snapshot = renderer.frame_snapshot();
            gpu.render_frame(&snapshot);
        }
    }

    fn request_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let attrs = Window::default_attributes().with_title("seance");
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
            ..Default::default()
        };
        let renderer =
            Renderer::new(&config, native_handle).expect("failed to create renderer");

        remove_ghostty_layer(&window);

        let gpu = pollster::block_on(GpuState::new(window.clone()));

        self.renderer = Some(renderer);
        self.gpu = Some(gpu);
        self.window = Some(window);

        // Get actual cell size from the renderer before spawning panes.
        // Do a throwaway update_frame with no terminal to read cell metrics.
        // Cell size comes from font metrics and is constant.
        if let Some(r) = &self.renderer {
            // Temporarily create a terminal to get cell size from frame_data.
            let tmp = Terminal::new(80, 24).expect("failed to create temp terminal");
            r.set_terminal(&tmp);
            r.update_frame();
            let snap = r.frame_snapshot();
            let fd = snap.frame_data();
            self.cell_size = [fd.cell_width, fd.cell_height];
            drop(snap);
            // Terminal is dropped here; we'll set the real one below.
        }

        let (cols, rows) = self.grid_size_for_pixels(size.width, size.height);
        self.spawn_pane(0, cols, rows);

        if let Some(pane) = self.panes.get(&0) {
            if let Some(r) = &self.renderer {
                r.set_terminal(&pane.terminal);
                r.set_background(0x1e, 0x1e, 0x2e);
            }
        }
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
                self.resize_panes(cols, rows);
                // Don't set needs_redraw here. The terminal was just
                // resized and cleared — the shell hasn't redrawn yet.
                // Let the PTY data flow trigger update_frame after the
                // shell finishes its SIGWINCH redraw.
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
                let action = self.input.handle_key(&event, &self.modifiers, self.focused_term_modes());
                match action {
                    Action::WritePty(data) => {
                        if let Some(pane) = self.panes.get(&self.focused) {
                            let _ = pane.pty.write_all(&data);
                        }
                    }
                    Action::Ignore => {}
                    _ => {
                        log::debug!("unhandled action: {action:?}");
                    }
                }
                self.needs_redraw = true;
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
                    let modes = self.focused_term_modes();
                    if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
                        if let Some(pane) = self.panes.get(&self.focused) {
                            let _ = pane.pty.write_all(&data);
                        }
                    } else if let Some(pane) = self.panes.get(&self.focused) {
                        pane.terminal.scroll(
                            ghostty_renderer::ScrollAction::Lines(-lines),
                        );
                    }
                    self.needs_redraw = true;
                    self.request_redraw();
                }
            }

            WindowEvent::RedrawRequested => {
                self.draw();
                // Keep polling for PTY output. request_redraw is
                // throttled by vsync so this won't spin.
                self.request_redraw();
            }

            _ => {}
        }
    }
}

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

/// Find wgpu's CAMetalLayer (a sublayer of the view's backing layer)
/// and call `f` with it. No-op if not found.
#[cfg(target_os = "macos")]
fn with_metal_layer(window: &Option<Arc<Window>>, f: impl FnOnce(*mut objc2::runtime::AnyObject)) {
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, AnyObject};

    let Some(window) = window else { return };
    let nsview = get_native_handle(window);
    unsafe {
        let view: *mut AnyObject = nsview.cast();
        let layer: *mut AnyObject = msg_send![view, layer];
        if layer.is_null() { return; }
        let sublayers: *mut AnyObject = msg_send![layer, sublayers];
        if sublayers.is_null() { return; }
        let count: usize = msg_send![sublayers, count];
        let Some(metal_class) = AnyClass::get("CAMetalLayer") else { return };
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
    panic!("libghostty-renderer currently requires macOS");
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
