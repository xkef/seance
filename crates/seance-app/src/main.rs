use std::collections::HashMap;
use std::sync::Arc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::application::ApplicationHandler;
use winit::event::{Modifiers, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use ghostty_renderer::{Renderer, RendererConfig, Terminal};
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
            match pane.pty.read(&mut buf) {
                Ok(n) if n > 0 => {
                    pane.terminal.vt_write(&buf[..n]);
                }
                _ => {}
            }
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

        let nsview = get_nsview(&window);

        let renderer = Renderer::new(&RendererConfig {
            nsview,
            content_scale: scale,
            width: size.width,
            height: size.height,
            font_size: 0.0,
        })
        .expect("failed to create renderer");

        self.renderer = Some(renderer);
        self.window = Some(window);

        self.spawn_pane(0, DEFAULT_COLS, DEFAULT_ROWS);

        if let Some(pane) = self.panes.get(&0) {
            if let Some(r) = &self.renderer {
                r.set_terminal(&pane.terminal);
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

                if let Some(r) = &self.renderer {
                    r.update_frame();
                    r.draw_frame();
                }

                self.request_redraw();
            }

            _ => {}
        }
    }
}

#[cfg(target_os = "macos")]
fn get_nsview(window: &Window) -> *mut std::ffi::c_void {
    let handle = window.window_handle().expect("no window handle");
    match handle.as_raw() {
        RawWindowHandle::AppKit(h) => h.ns_view.as_ptr(),
        _ => panic!("expected AppKit window handle"),
    }
}

#[cfg(not(target_os = "macos"))]
fn get_nsview(_window: &Window) -> *mut std::ffi::c_void {
    panic!("libghostty-renderer currently requires macOS");
}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
