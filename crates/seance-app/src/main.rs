use std::collections::HashMap;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use ghostty_renderer::{Renderer, RendererConfig, Terminal};
use seance_gpu::GpuState;
use seance_input::{Action, InputHandler};
use seance_layout::{LayoutTree, PaneId};
use seance_pty::Pty;

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;
const CELL_WIDTH: f32 = 8.0;
const CELL_HEIGHT: f32 = 16.0;

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
}

impl App {
    fn new() -> Self {
        Self {
            renderer: None,
            gpu: None,
            window: None,
            panes: HashMap::new(),
            layout: LayoutTree::new(0, CELL_WIDTH, CELL_HEIGHT),
            focused: 0,
            input: InputHandler::new(),
            modifiers: Modifiers::default(),
        }
    }

    fn spawn_pane(&mut self, id: PaneId, cols: u16, rows: u16) {
        let terminal = Terminal::new(cols, rows).expect("failed to create terminal");
        let pty = Pty::spawn(seance_pty::Size { cols, rows }).expect("failed to spawn PTY");
        self.panes.insert(id, Pane { terminal, pty });
    }

    fn pump_pty_io(&mut self) {
        let mut buf = [0u8; 4096];
        let ids: Vec<PaneId> = self.panes.keys().copied().collect();
        for id in ids {
            let pane = self.panes.get_mut(&id).unwrap();

            loop {
                match pane.pty.read(&mut buf) {
                    Ok(n) if n > 0 => pane.terminal.vt_write(&buf[..n]),
                    _ => break,
                }
            }

            let responses = pane.terminal.drain_responses();
            if !responses.is_empty() {
                let _ = pane.pty.write_all(responses);
            }
            pane.terminal.clear_responses();
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

        // Remove ghostty's IOSurfaceLayer so it doesn't cover our wgpu surface.
        // ghostty attached it during Metal init; we only need the CPU-side
        // cell buffers and atlas, not the Metal rendering.
        remove_ghostty_layer(&window);

        // Create wgpu state on the real window.
        let gpu = pollster::block_on(GpuState::new(window.clone()));

        self.renderer = Some(renderer);
        self.gpu = Some(gpu);
        self.window = Some(window);

        self.spawn_pane(0, DEFAULT_COLS, DEFAULT_ROWS);

        if let Some(pane) = self.panes.get(&0) {
            if let Some(r) = &self.renderer {
                r.set_terminal(&pane.terminal);
                // Catppuccin Mocha (hardcoded until theme loading works).
                // Must be called AFTER set_terminal so colors propagate.
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
                self.request_redraw();
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let action = self.input.handle_key(&event, &self.modifiers);
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
                self.request_redraw();
            }

            WindowEvent::RedrawRequested => {
                self.pump_pty_io();

                if let (Some(renderer), Some(gpu)) = (&self.renderer, &mut self.gpu) {
                    renderer.update_frame();
                    let snapshot = renderer.frame_snapshot();

                    let fd = snapshot.frame_data();
                    let bg = snapshot.bg_cells();
                    let text = snapshot.text_cells();
                    let atlas_gs = snapshot.atlas_grayscale();
                    let atlas_c = snapshot.atlas_color();

                    static FRAME: std::sync::atomic::AtomicU32 =
                        std::sync::atomic::AtomicU32::new(0);
                    let frame = FRAME.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if frame < 3 || frame % 300 == 0 {
                        log::info!(
                            "frame={} grid={}x{} cell={:.1}x{:.1} text={} atlas={}px",
                            frame,
                            fd.grid_cols, fd.grid_rows,
                            fd.cell_width, fd.cell_height,
                            text.len(),
                            atlas_gs.size,
                        );
                    }

                    gpu.render_frame(&snapshot);
                }

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
    // SAFETY: ghostty attached an IOSurfaceLayer during Metal init.
    // Remove it so wgpu can create its own layer on the same view.
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
