use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{ElementState, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use seance_input::{Action, InputHandler};
use seance_terminal::{RendererConfig, Terminal, TerminalRenderer};

mod platform;

const DEFAULT_FONT_SIZE: f32 = 14.0;

struct MouseState {
    cursor_pos: winit::dpi::PhysicalPosition<f64>,
    is_down: bool,
    click_count: u8,
    last_click_time: Instant,
    last_click_pos: (u16, u16),
}

impl Default for MouseState {
    fn default() -> Self {
        Self {
            cursor_pos: winit::dpi::PhysicalPosition::new(0.0, 0.0),
            is_down: false,
            click_count: 0,
            last_click_time: Instant::now(),
            last_click_pos: (0, 0),
        }
    }
}

impl MouseState {
    fn register_click(&mut self, col: u16, row: u16) -> u8 {
        let now = Instant::now();
        let dt = now.duration_since(self.last_click_time);
        if dt.as_millis() < 500 && (col, row) == self.last_click_pos {
            self.click_count = (self.click_count % 3) + 1;
        } else {
            self.click_count = 1;
        }
        self.last_click_time = now;
        self.last_click_pos = (col, row);
        self.click_count
    }
}

struct App {
    window: Option<Arc<Window>>,
    renderer: Option<TerminalRenderer>,
    terminal: Option<Terminal>,
    input: InputHandler,
    modifiers: Modifiers,
    cell_size: [f32; 2],
    font_size: f32,
    content_dirty: bool,
    occluded: bool,
    mouse: MouseState,
}

impl App {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            terminal: None,
            input: InputHandler::new(),
            modifiers: Modifiers::default(),
            cell_size: [0.0, 0.0],
            font_size: DEFAULT_FONT_SIZE,
            content_dirty: true,
            occluded: false,
            mouse: MouseState::default(),
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn poll_pty(&mut self) -> bool {
        let Some(term) = &mut self.terminal else {
            return false;
        };
        let got_data = term.poll();
        if !term.is_alive() {
            self.terminal = None;
        }
        if got_data {
            self.content_dirty = true;
        }
        got_data
    }

    fn draw(&mut self) {
        if self.occluded || self.terminal.is_none() {
            return;
        }
        if self.content_dirty {
            self.content_dirty = false;
            if let (Some(r), Some(t)) = (&mut self.renderer, &mut self.terminal) {
                r.update_frame(t);
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
            let (cols, rows) = r.grid_size();
            if let Some(term) = &mut self.terminal {
                term.resize(cols, rows, size.width as u16, size.height as u16);
            }
        }
        self.content_dirty = true;
        self.request_redraw();
    }

    fn terminal_modes(&self) -> seance_input::TerminalModes {
        self.terminal
            .as_ref()
            .map_or(Default::default(), |t| t.modes())
    }

    fn clear_selection(&mut self) {
        if let Some(term) = &mut self.terminal {
            term.clear_selection();
        }
        if let Some(r) = &mut self.renderer {
            r.overlay_mut().selection = None;
        }
    }

    fn sync_selection_to_overlay(&mut self) {
        let range = self.terminal.as_ref().and_then(|t| t.selection_range());
        if let Some(r) = &mut self.renderer {
            r.overlay_mut().selection = range;
        }
    }

    fn on_resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            r.resize_surface(new_size.width, new_size.height, w.scale_factor());
        }
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((80, 24));
        if let Some(term) = &mut self.terminal {
            term.resize(cols, rows, new_size.width as u16, new_size.height as u16);
        }
        self.content_dirty = true;
        self.draw();
    }

    fn on_keyboard_input(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        let modes = self.terminal_modes();
        let action = self.input.handle_key(event, &self.modifiers, modes);

        if event.state == ElementState::Pressed
            && !matches!(action, Action::Copy | Action::SelectAll | Action::Ignore)
            && self.terminal.as_ref().is_some_and(|t| t.has_selection())
        {
            self.clear_selection();
        }

        self.execute_action(event_loop, action);
        self.content_dirty = true;
        self.request_redraw();
    }

    fn execute_action(&mut self, event_loop: &ActiveEventLoop, action: Action) {
        match action {
            Action::WritePty(data) => {
                if let Some(term) = &self.terminal {
                    term.write(&data);
                }
            }
            Action::Quit | Action::CloseWindow => {
                event_loop.exit();
            }
            Action::Copy => {
                if let Some(term) = &mut self.terminal
                    && let Some(text) = term.selection_text()
                    && let Ok(mut cb) = arboard::Clipboard::new()
                {
                    let _ = cb.set_text(text);
                }
                self.clear_selection();
            }
            Action::Paste => {
                if let Ok(mut cb) = arboard::Clipboard::new()
                    && let Ok(text) = cb.get_text()
                {
                    let bracketed = self
                        .terminal
                        .as_ref()
                        .is_some_and(|t| t.modes().bracketed_paste);
                    if let Some(term) = &self.terminal {
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
            Action::SelectAll => {
                if let Some(term) = &mut self.terminal {
                    term.select_all();
                }
                self.sync_selection_to_overlay();
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
        }
    }

    fn on_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                let ch = self.cell_size[1].max(1.0);
                (pos.y / ch as f64) as i32
            }
        };
        if lines == 0 {
            return;
        }
        let modes = self.terminal_modes();
        if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
            if let Some(term) = &self.terminal {
                term.write(&data);
            }
        } else if let Some(term) = &mut self.terminal {
            term.scroll_lines(-lines);
        }
        self.content_dirty = true;
        self.request_redraw();
    }

    fn on_cursor_moved(&mut self, position: winit::dpi::PhysicalPosition<f64>) {
        self.mouse.cursor_pos = position;
        if !self.mouse.is_down {
            return;
        }
        let grid_pos = self
            .renderer
            .as_ref()
            .map(|r| r.pixel_to_grid(position.x, position.y));
        if let Some((col, row)) = grid_pos {
            if let Some(term) = &mut self.terminal {
                term.update_selection(col, row);
            }
            self.sync_selection_to_overlay();
            self.content_dirty = true;
            self.request_redraw();
        }
    }

    fn on_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        match state {
            ElementState::Pressed => {
                let grid_pos = self
                    .renderer
                    .as_ref()
                    .map(|r| r.pixel_to_grid(self.mouse.cursor_pos.x, self.mouse.cursor_pos.y));
                if let Some((col, row)) = grid_pos {
                    let clicks = self.mouse.register_click(col, row);
                    if let Some(term) = &mut self.terminal {
                        match clicks {
                            1 => term.start_selection(col, row),
                            2 => term.start_word_selection(col, row),
                            3 => term.start_line_selection(row),
                            _ => {}
                        }
                    }
                    self.sync_selection_to_overlay();
                }
                self.mouse.is_down = true;
                self.content_dirty = true;
                self.request_redraw();
            }
            ElementState::Released => {
                self.mouse.is_down = false;
                if let Some(term) = &mut self.terminal
                    && let Some(text) = term.selection_text()
                    && !text.is_empty()
                    && let Ok(mut cb) = arboard::Clipboard::new()
                {
                    let _ = cb.set_text(text);
                }
            }
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

        #[cfg(target_os = "macos")]
        platform::configure_window(&window);

        let size = window.inner_size();
        let scale = window.scale_factor();

        let config = RendererConfig {
            width: size.width,
            height: size.height,
            scale,
        };

        let renderer = pollster::block_on(TerminalRenderer::new(window.clone(), config))
            .expect("failed to create renderer");

        self.cell_size = renderer.cell_size();
        let (cols, rows) = renderer.grid_size();
        self.renderer = Some(renderer);
        self.window = Some(window);

        let term = Terminal::spawn(cols, rows, size.width as u16, size.height as u16)
            .expect("failed to spawn terminal");
        self.terminal = Some(term);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => self.on_resize(size),
            WindowEvent::ModifiersChanged(mods) => self.modifiers = mods,
            WindowEvent::KeyboardInput { event, .. } => {
                self.on_keyboard_input(event_loop, &event);
            }
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_input(state, button),
            WindowEvent::Occluded(is_occluded) => {
                self.occluded = is_occluded;
                if !is_occluded {
                    self.content_dirty = true;
                    self.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.terminal.is_none() {
            event_loop.exit();
            return;
        }

        self.poll_pty();

        if self.terminal.is_none() {
            event_loop.exit();
            return;
        }

        if self.content_dirty && !self.occluded {
            self.request_redraw();
        }

        event_loop.set_control_flow(winit::event_loop::ControlFlow::wait_duration(
            std::time::Duration::from_millis(4),
        ));
    }
}

fn main() {
    env_logger::init();
    let mut builder = EventLoop::builder();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Regular);
    }
    let event_loop = builder.build().expect("failed to create event loop");
    let mut app = App::new();
    event_loop.run_app(&mut app).expect("event loop failed");
}
