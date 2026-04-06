use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use seance_input::{Action, InputHandler};
use seance_layout::{LayoutTree, PaneId};
use seance_terminal::{RendererConfig, Terminal, TerminalRenderer};

const DEFAULT_FONT_SIZE: f32 = 14.0;

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<TerminalRenderer>,
    panes: HashMap<PaneId, Terminal>,
    layout: LayoutTree,
    focused: PaneId,
    input: InputHandler,
    modifiers: Modifiers,
    cell_size: [f32; 2],
    font_size: f32,
    content_dirty: bool,
    cursor_pos: winit::dpi::PhysicalPosition<f64>,
    mouse_down: bool,
    click_count: u8,
    last_click_time: Instant,
    last_click_pos: (u16, u16),
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
            font_size: DEFAULT_FONT_SIZE,
            content_dirty: true,
            cursor_pos: winit::dpi::PhysicalPosition::new(0.0, 0.0),
            mouse_down: false,
            click_count: 0,
            last_click_time: Instant::now(),
            last_click_pos: (0, 0),
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn poll_pty(&mut self) -> bool {
        let mut got_data = false;
        let mut dead = Vec::new();
        for (&id, term) in self.panes.iter_mut() {
            got_data |= term.poll();
            if !term.is_alive() {
                dead.push(id);
            }
        }
        for id in dead {
            self.panes.remove(&id);
        }
        if got_data {
            self.content_dirty = true;
        }
        got_data
    }

    fn draw(&mut self) {
        if self.panes.is_empty() {
            return;
        }

        if self.content_dirty {
            self.content_dirty = false;
            if let Some(r) = &self.renderer {
                r.update_frame();
            }
        }

        if let Some(r) = &mut self.renderer {
            r.render();
        }
    }

    fn apply_font_size(&mut self) {
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            r.set_font_size(self.font_size);
            let size = w.inner_size();
            r.resize_surface(size.width, size.height, w.scale_factor());
            self.cell_size = r.cell_size();
            self.layout
                .set_cell_size(self.cell_size[0], self.cell_size[1]);
            let (cols, rows) = r.grid_size();
            for term in self.panes.values_mut() {
                term.resize(cols, rows);
            }
        }
        self.content_dirty = true;
        self.request_redraw();
    }
}

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

        #[cfg(target_os = "macos")]
        configure_macos_window(&window);

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

        let theme = std::env::var("SEANCE_THEME")
            .unwrap_or_else(|_| "catppuccin-frappe".to_string());
        if !renderer.set_theme(&theme) {
            log::warn!("failed to load theme: {theme}");
        }

        self.cell_size = renderer.cell_size();
        self.layout
            .set_cell_size(self.cell_size[0], self.cell_size[1]);
        let (cols, rows) = renderer.grid_size();
        self.renderer = Some(renderer);
        self.window = Some(window);

        let term = Terminal::spawn(cols, rows).expect("failed to spawn terminal");
        if let Some(r) = &self.renderer {
            r.attach(&term);
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
                let (cols, rows) = self
                    .renderer
                    .as_ref()
                    .map(|r| r.grid_size())
                    .unwrap_or((80, 24));
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
                let modes = self
                    .panes
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

                // Clear selection on keypress (except Cmd+C and modifiers)
                if event.state == ElementState::Pressed
                    && !matches!(action, Action::Copy | Action::SelectAll | Action::Ignore)
                {
                    let had_selection = self
                        .panes
                        .get(&self.focused)
                        .map_or(false, |t| t.has_selection());
                    if had_selection {
                        if let Some(term) = self.panes.get_mut(&self.focused) {
                            term.clear_selection();
                        }
                        if let Some(r) = &mut self.renderer {
                            r.overlay_mut().selection = None;
                        }
                    }
                }
                match action {
                    Action::WritePty(data) => {
                        if let Some(term) = self.panes.get(&self.focused) {
                            term.write(&data);
                        }
                    }
                    Action::Quit | Action::CloseWindow => {
                        event_loop.exit();
                    }
                    Action::Copy => {
                        if let Some(term) = self.panes.get_mut(&self.focused) {
                            if let Some(text) = term.selection_text() {
                                if let Ok(mut cb) = arboard::Clipboard::new() {
                                    let _ = cb.set_text(text);
                                }
                            }
                            term.clear_selection();
                        }
                        if let Some(r) = &mut self.renderer {
                            r.overlay_mut().selection = None;
                        }
                    }
                    Action::Paste => {
                        if let Ok(mut cb) = arboard::Clipboard::new() {
                            if let Ok(text) = cb.get_text() {
                                let bracketed = self
                                    .panes
                                    .get(&self.focused)
                                    .map_or(false, |t| t.modes().bracketed_paste);
                                if let Some(term) = self.panes.get(&self.focused) {
                                    if bracketed {
                                        term.write(b"\x1b[200~");
                                    }
                                    term.write(text.as_bytes());
                                    if bracketed {
                                        term.write(b"\x1b[201~");
                                    }
                                }
                            }
                        }
                    }
                    Action::SelectAll => {
                        if let Some(term) = self.panes.get_mut(&self.focused) {
                            term.select_all();
                            let range = term.selection_range();
                            if let Some(r) = &mut self.renderer {
                                r.overlay_mut().selection = range;
                            }
                        }
                    }
                    Action::IncreaseFontSize => {
                        self.font_size = (self.font_size + 1.0).min(72.0);
                        self.apply_font_size();
                    }
                    Action::DecreaseFontSize => {
                        self.font_size = (self.font_size - 1.0).max(6.0);
                        self.apply_font_size();
                    }
                    Action::ResetFontSize => {
                        self.font_size = DEFAULT_FONT_SIZE;
                        self.apply_font_size();
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
                    let modes = self
                        .panes
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
                        term.scroll_lines(-lines);
                    }
                    self.content_dirty = true;
                    self.request_redraw();
                }
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor_pos = position;
                if self.mouse_down {
                    let grid_pos = self
                        .renderer
                        .as_ref()
                        .map(|r| r.pixel_to_grid(position.x, position.y));
                    if let Some((col, row)) = grid_pos {
                        if let Some(term) = self.panes.get_mut(&self.focused) {
                            term.update_selection(col, row);
                        }
                        let range = self
                            .panes
                            .get(&self.focused)
                            .and_then(|t| t.selection_range());
                        if let Some(r) = &mut self.renderer {
                            r.overlay_mut().selection = range;
                        }
                        self.content_dirty = true;
                        self.request_redraw();
                    }
                }
            }

            WindowEvent::MouseInput { state, button, .. } => {
                if button == MouseButton::Left {
                    match state {
                        ElementState::Pressed => {
                            let now = Instant::now();
                            let grid_pos = self
                                .renderer
                                .as_ref()
                                .map(|r| r.pixel_to_grid(self.cursor_pos.x, self.cursor_pos.y));
                            if let Some((col, row)) = grid_pos {
                                let dt = now.duration_since(self.last_click_time);
                                if dt.as_millis() < 500
                                    && (col, row) == self.last_click_pos
                                {
                                    self.click_count = (self.click_count % 3) + 1;
                                } else {
                                    self.click_count = 1;
                                }
                                self.last_click_time = now;
                                self.last_click_pos = (col, row);

                                if let Some(term) = self.panes.get_mut(&self.focused) {
                                    match self.click_count {
                                        1 => term.start_selection(col, row),
                                        2 => term.start_word_selection(col, row),
                                        3 => term.start_line_selection(row),
                                        _ => {}
                                    }
                                    let range = term.selection_range();
                                    if let Some(r) = &mut self.renderer {
                                        r.overlay_mut().selection = range;
                                    }
                                }
                            }
                            self.mouse_down = true;
                            self.content_dirty = true;
                            self.request_redraw();
                        }
                        ElementState::Released => {
                            self.mouse_down = false;
                            // Auto-copy selection to clipboard on mouse release
                            if let Some(term) = self.panes.get_mut(&self.focused) {
                                if let Some(text) = term.selection_text() {
                                    if !text.is_empty() {
                                        if let Ok(mut cb) = arboard::Clipboard::new() {
                                            let _ = cb.set_text(text);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                self.draw();
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.panes.is_empty() {
            event_loop.exit();
            return;
        }

        self.poll_pty();

        if self.panes.is_empty() {
            event_loop.exit();
            return;
        }

        if self.content_dirty {
            self.request_redraw();
        }

        event_loop.set_control_flow(winit::event_loop::ControlFlow::wait_duration(
            std::time::Duration::from_millis(4),
        ));
    }
}

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn configure_macos_window(window: &Window) {
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
        let nswindow: *mut AnyObject = msg_send![view, window];
        if nswindow.is_null() {
            return;
        }
        let mask: usize = 1 | 2 | 4 | 8 | (1 << 15);
        let _: () = msg_send![nswindow, setStyleMask: mask];
        let _: () = msg_send![nswindow, setTitlebarAppearsTransparent: true];
        let _: () = msg_send![nswindow, setTitleVisibility: 1_isize];
        let _: () = msg_send![nswindow, setMovableByWindowBackground: true];

        for i in 0_isize..3 {
            let button: *mut AnyObject = msg_send![nswindow, standardWindowButton: i];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: true];
            }
        }
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
    std::ptr::null_mut()
}

fn main() {
    env_logger::init();

    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let resources = manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("ghostty/zig-out/share/ghostty");
    unsafe { std::env::set_var("GHOSTTY_RESOURCES_DIR", &resources) };

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
