use std::collections::HashMap;
use std::sync::Arc;

use winit::application::ApplicationHandler;
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use seance_input::{Action, InputHandler};
use seance_layout::{LayoutTree, PaneId};
use seance_terminal::{Color, RendererConfig, ScrollAction, Terminal, TerminalRenderer};

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<TerminalRenderer>,

    // Panes
    panes: HashMap<PaneId, Terminal>,
    layout: LayoutTree,
    focused: PaneId,

    // Input
    input: InputHandler,
    modifiers: Modifiers,

    // Metrics (constant after init)
    cell_size: [f32; 2],

    // Frame scheduling
    content_dirty: bool,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            panes: HashMap::new(),
            layout: LayoutTree::new(0, 1.0, 1.0),
            focused: 0,
            input: InputHandler::new(),
            modifiers: Modifiers::default(),
            cell_size: [0.0, 0.0],
            content_dirty: true,
        }
    }

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

    fn poll_pty(&mut self) -> bool {
        let mut got_data = false;
        for term in self.panes.values_mut() {
            got_data |= term.poll();
        }
        got_data
    }

    fn draw(&mut self) {
        let got_data = self.poll_pty();

        if got_data || self.content_dirty {
            self.content_dirty = false;
            if let Some(r) = &self.renderer {
                r.update_frame();
            }
        }

        if let Some(r) = &mut self.renderer {
            r.render();
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

        let config = RendererConfig {
            width: size.width,
            height: size.height,
            scale,
            native_handle: get_native_handle(&window),
        };

        let renderer = pollster::block_on(TerminalRenderer::new(window.clone(), config))
            .expect("failed to create renderer");

        self.cell_size = renderer.cell_size();
        self.layout.set_cell_size(self.cell_size[0], self.cell_size[1]);
        self.renderer = Some(renderer);
        self.window = Some(window);

        let (cols, rows) = self.grid_size_for_pixels(size.width, size.height);
        let term = Terminal::spawn(cols, rows).expect("failed to spawn terminal");

        if let Some(r) = &self.renderer {
            r.attach(&term);
            r.set_background(Color { r: 0x30, g: 0x34, b: 0x46 });
        }

        self.panes.insert(0, term);
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
                if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
                    r.resize_surface(new_size.width, new_size.height, w.scale_factor());
                }
                let (cols, rows) = self.grid_size_for_pixels(new_size.width, new_size.height);
                for term in self.panes.values_mut() {
                    term.resize(cols, rows);
                }
                self.content_dirty = true;
                self.draw();
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.modifiers = mods;
            }

            WindowEvent::KeyboardInput { event, .. } => {
                let modes = self.panes
                    .get(&self.focused)
                    .map_or(Default::default(), |t| {
                        let m = t.modes();
                        seance_input::TerminalModes {
                            cursor_keys: m.cursor_keys,
                            mouse_event: m.mouse_event,
                            mouse_format_sgr: m.mouse_format_sgr,
                        }
                    });
                let action = self.input.handle_key(&event, &self.modifiers, modes);
                match action {
                    Action::WritePty(data) => {
                        if let Some(term) = self.panes.get(&self.focused) {
                            term.write(&data);
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
                        .map_or(Default::default(), |t| {
                            let m = t.modes();
                            seance_input::TerminalModes {
                                cursor_keys: m.cursor_keys,
                                mouse_event: m.mouse_event,
                                mouse_format_sgr: m.mouse_format_sgr,
                            }
                        });
                    if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
                        if let Some(term) = self.panes.get(&self.focused) {
                            term.write(&data);
                        }
                    } else if let Some(term) = self.panes.get_mut(&self.focused) {
                        term.scroll(ScrollAction::Lines(-lines));
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
    std::ptr::null_mut()
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
