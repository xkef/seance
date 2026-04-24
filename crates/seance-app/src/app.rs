//! `App` — the top-level winit `ApplicationHandler`.
//!
//! Owns process-lifetime state (config, input handler, config watcher) and
//! a single `window_state: Option<WindowState>` for everything that exists
//! only while a window is up. Hot-reload methods live in the `reload` module.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Window, WindowId};

use seance_config::Config;
use seance_input::{InputHandler, VtInput};
use seance_render::{RenderInputs, RendererConfig, TerminalRenderer};
use seance_vt::{CursorShape as VtCursorShape, FrameSource, LibGhosttyFrameSource, Terminal};

use crate::UserEvent;
use crate::command::AppCommand;
use crate::keybinds::Keybinds;
use crate::platform;
use crate::watcher::ConfigWatcher;
use crate::window_state::WindowState;

const FONT_SIZE_MIN: f32 = 6.0;
const FONT_SIZE_MAX: f32 = 72.0;
const POLL_INTERVAL: Duration = Duration::from_millis(4);
// Half-period of the cursor blink cycle; on + off = 1 s. The tick rides on
// POLL_INTERVAL wakeups — once M2 #24 lands we should drive it from the
// deadline scheduler instead.
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);

pub(crate) struct App {
    pub(crate) window_state: Option<WindowState>,
    pub(crate) input: InputHandler,
    keybinds: Keybinds,
    pub(crate) config: Config,
    pub(crate) font_size: f32,
    proxy: EventLoopProxy<UserEvent>,
    watcher: Option<ConfigWatcher>,
}

impl App {
    pub(crate) fn new(config: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        let font_size = config.font.size;
        let mut input = InputHandler::new();
        input.set_option_as_alt(platform::option_as_alt_from_config(
            config.input.macos_option_as_alt,
        ));
        Self {
            window_state: None,
            input,
            keybinds: Keybinds::new(),
            config,
            font_size,
            proxy,
            watcher: None,
        }
    }

    /// Shortcut — most methods run only while a window is up.
    pub(crate) fn ws_mut(&mut self) -> Option<&mut WindowState> {
        self.window_state.as_mut()
    }

    pub(crate) fn mark_dirty(&mut self) {
        if let Some(ws) = self.ws_mut() {
            ws.mark_dirty();
        }
    }

    fn draw(&mut self) {
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        if ws.occluded {
            return;
        }
        if ws.content_dirty {
            ws.content_dirty = false;
            let mut source = LibGhosttyFrameSource::new(&mut ws.terminal);
            // Cache the VT's DECSCUSR-tracked shape (if any) before the
            // renderer consumes the source — mode changes in neovim arrive
            // as PTY bytes that set `content_dirty`, so this branch runs
            // on every mode transition.
            ws.last_vt_cursor_shape = source.cursor().shape;
            ws.renderer.update_frame(&mut source);
        }
        // Prefer the VT-reported shape; fall back to the user's configured
        // default when the VT has no opinion. Refreshed every frame so that
        // hot-reload of `cursor.style` is picked up without extra wiring.
        ws.render_inputs.cursor_shape = ws
            .last_vt_cursor_shape
            .map(Into::into)
            .unwrap_or_else(|| self.config.cursor.style.into());
        ws.render_inputs.vt_cursor_visible = !self.config.cursor.blink || ws.blink_on;
        ws.renderer.render(&ws.render_inputs);
    }

    fn tick_blink(&mut self) {
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        if !self.config.cursor.blink {
            if !ws.blink_on {
                ws.blink_on = true;
                ws.mark_dirty();
            }
            return;
        }
        if ws.last_blink_edge.elapsed() >= BLINK_HALF_PERIOD {
            ws.blink_on = !ws.blink_on;
            ws.last_blink_edge = Instant::now();
            ws.mark_dirty();
        }
    }

    fn apply_font_size(&mut self) {
        let font_size = self.font_size;
        if let Some(ws) = self.ws_mut() {
            ws.renderer.set_font_size(font_size);
            ws.reflow(ws.window.inner_size());
        }
    }

    pub(crate) fn apply_font_metrics(
        &mut self,
        font_size_changed: bool,
        adjust_cell_height_changed: bool,
    ) {
        let font_size = self.font_size;
        let adjust = self.config.font.adjust_cell_height.clone();
        if let Some(ws) = self.ws_mut() {
            if adjust_cell_height_changed {
                ws.renderer.set_adjust_cell_height(adjust.as_deref());
            }
            if font_size_changed {
                ws.renderer.set_font_size(font_size);
            }
            ws.reflow(ws.window.inner_size());
        }
    }

    fn apply_scale_factor(&mut self, scale_factor: f64) {
        let padding = physical_window_padding(&self.config, scale_factor);
        if let Some(ws) = self.ws_mut() {
            ws.renderer.set_scale(scale_factor);
            ws.renderer.set_window_padding(padding);
            ws.reflow(ws.window.inner_size());
        }
    }

    /// Push the configured window padding to the renderer and reflow the PTY.
    /// `grid_size()` shrinks when padding grows, so a reflow is required to
    /// keep the shell's SIGWINCH in sync.
    pub(crate) fn apply_window_padding(&mut self) {
        let config = &self.config;
        if let Some(ws) = self.window_state.as_mut() {
            let padding = physical_window_padding(config, ws.window.scale_factor());
            ws.renderer.set_window_padding(padding);
            ws.reflow(ws.window.inner_size());
        }
    }

    fn on_keyboard_input(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        let modes = self
            .window_state
            .as_ref()
            .map(|ws| ws.terminal_modes())
            .unwrap_or_default();
        let modifiers = self
            .window_state
            .as_ref()
            .map(|ws| ws.modifiers)
            .unwrap_or_default();

        if let Some(cmd) = self.keybinds.match_event(event, &modifiers) {
            let preserves_selection = matches!(cmd, AppCommand::Copy | AppCommand::SelectAll);
            if !preserves_selection
                && let Some(ws) = self.ws_mut()
                && ws.has_selection()
            {
                ws.clear_selection();
            }
            self.execute_app_command(event_loop, cmd);
            self.mark_dirty();
            return;
        }

        let input = self.input.handle_key(event, &modifiers, modes);

        if let Some(ws) = self.ws_mut() {
            if event.state == ElementState::Pressed
                && !matches!(input, VtInput::Ignore)
                && ws.has_selection()
            {
                ws.clear_selection();
            }
            if let VtInput::Write(bytes) = input {
                ws.terminal.write(&bytes);
            }
            ws.mark_dirty();
        }
    }

    fn execute_app_command(&mut self, event_loop: &ActiveEventLoop, cmd: AppCommand) {
        match cmd {
            AppCommand::Quit | AppCommand::CloseWindow => event_loop.exit(),
            AppCommand::Copy => {
                if let Some(ws) = self.ws_mut() {
                    ws.copy_selection_to_clipboard();
                    ws.clear_selection();
                }
            }
            AppCommand::Paste => {
                if let Some(ws) = self.ws_mut() {
                    ws.paste_from_clipboard();
                }
            }
            AppCommand::SelectAll => {
                if let Some(ws) = self.ws_mut() {
                    ws.terminal.select_all();
                    ws.sync_selection_to_overlay();
                }
            }
            AppCommand::FontSizeDelta(delta) => {
                self.font_size =
                    (self.font_size + f32::from(delta)).clamp(FONT_SIZE_MIN, FONT_SIZE_MAX);
                self.apply_font_size();
            }
            AppCommand::FontSizeReset => {
                self.font_size = self.config.font.size;
                self.apply_font_size();
            }
        }
    }

    fn on_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                let ch = ws.cell_size[1].max(1.0);
                (pos.y / f64::from(ch)) as i32
            }
        };
        if lines == 0 {
            return;
        }
        let modes = ws.terminal_modes();
        if let Some(data) = self.input.encode_mouse_wheel(lines, modes) {
            ws.terminal.write(&data);
        } else {
            ws.terminal.scroll_lines(-lines);
        }
        ws.mark_dirty();
    }

    fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        ws.mouse.cursor_pos = position;
        if !ws.mouse.is_down {
            return;
        }
        let (col, row) = ws.renderer.pixel_to_grid(position.x, position.y);
        ws.terminal.update_selection(col, row);
        ws.sync_selection_to_overlay();
        ws.mark_dirty();
    }

    fn on_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        let Some(ws) = self.window_state.as_mut() else {
            return;
        };
        match state {
            ElementState::Pressed => handle_mouse_press(ws),
            ElementState::Released => {
                ws.mouse.is_down = false;
                ws.copy_selection_to_clipboard();
            }
        }
    }
}

fn handle_mouse_press(ws: &mut WindowState) {
    if ws.modifiers.state().super_key() {
        let _ = ws.window.drag_window();
        return;
    }
    let (col, row) = ws
        .renderer
        .pixel_to_grid(ws.mouse.cursor_pos.x, ws.mouse.cursor_pos.y);
    let clicks = ws.mouse.register_click(col, row);
    match clicks {
        1 => ws.terminal.start_selection(col, row),
        2 => ws.terminal.start_word_selection(col, row),
        3 => ws.terminal.start_line_selection(row),
        _ => {}
    }
    ws.sync_selection_to_overlay();
    ws.mouse.is_down = true;
    ws.mark_dirty();
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window_state.is_some() {
            return;
        }

        let mut window_attrs = Window::default_attributes()
            .with_title("seance")
            .with_decorations(self.config.window.decoration);
        if let Some(size) = initial_window_size_from_env() {
            window_attrs = window_attrs.with_inner_size(size);
        }
        let window = Arc::new(
            event_loop
                .create_window(window_attrs)
                .expect("failed to create window"),
        );

        if self.config.window.decoration {
            platform::configure_window(&window);
        }
        platform::set_option_as_alt(
            &window,
            platform::option_as_alt_from_config(self.config.input.macos_option_as_alt),
        );

        let size = window.inner_size();
        let theme = seance_config::load_theme(self.config.theme.as_deref());
        let renderer_config = RendererConfig {
            width: size.width,
            height: size.height,
            scale: window.scale_factor(),
            font_family: self.config.font.family.clone(),
            font_size: self.font_size,
            adjust_cell_height: self.config.font.adjust_cell_height.clone(),
            min_contrast: self.config.font.min_contrast,
            window_padding: physical_window_padding(&self.config, window.scale_factor()),
            background_opacity: self.config.window.background_opacity,
            theme: theme.clone(),
        };

        let renderer = pollster::block_on(TerminalRenderer::new(window.clone(), renderer_config))
            .expect("failed to create renderer");
        platform::configure_metal_layer(&window);

        let (cols, rows) = renderer.grid_size();
        let mut term = Terminal::spawn(cols, rows, size.width as u16, size.height as u16)
            .expect("failed to spawn terminal");
        // Seed the VT's DECSCUSR state with the user's configured shape so
        // the bash prompt doesn't inherit ghostty's hardcoded `.block`
        // default. App-level DECSCUSR emissions (e.g. neovim mode changes)
        // still override on subsequent frames.
        term.set_cursor_shape(vt_shape_from_config(self.config.cursor.style));

        let render_inputs = RenderInputs {
            cursor_shape: self.config.cursor.style.into(),
            ..RenderInputs::default()
        };
        let mut ws = WindowState::new(window, renderer, term, render_inputs);
        self.apply_terminal_theme_to(&mut ws, &theme);
        self.window_state = Some(ws);

        // Start watching the config dir for edits. A non-XDG environment or
        // an unreadable dir just skips the watcher — seance keeps running.
        if self.watcher.is_none()
            && let Some(dir) = seance_config::config_dir()
        {
            self.watcher = ConfigWatcher::spawn(&dir, self.proxy.clone());
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ConfigFileChanged => self.reload_config(),
            UserEvent::ThemeFileChanged(path) => self.on_theme_file_changed(&path),
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
            WindowEvent::Resized(size) => {
                if let Some(ws) = self.ws_mut() {
                    ws.reflow(size);
                }
                self.draw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.apply_scale_factor(scale_factor);
                self.draw();
            }
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(ws) = self.ws_mut() {
                    ws.modifiers = mods;
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.on_keyboard_input(event_loop, &event),
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_input(state, button),
            WindowEvent::Occluded(is_occluded) => {
                if let Some(ws) = self.ws_mut() {
                    ws.occluded = is_occluded;
                    if !is_occluded {
                        ws.mark_dirty();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(ws) = self.ws_mut()
            && !ws.poll_pty()
        {
            self.window_state = None;
        }
        if self.window_state.is_none() {
            event_loop.exit();
            return;
        }
        self.tick_blink();
        if let Some(ws) = self.window_state.as_ref()
            && ws.content_dirty
            && !ws.occluded
        {
            ws.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::wait_duration(POLL_INTERVAL));
    }
}

fn initial_window_size_from_env() -> Option<LogicalSize<u32>> {
    let value = std::env::var("SEANCE_INITIAL_WINDOW_SIZE").ok()?;
    let (width, height) = value.split_once(',').or_else(|| value.split_once('x'))?;
    let width = width.parse().ok()?;
    let height = height.parse().ok()?;
    Some(LogicalSize::new(width, height))
}

pub(crate) fn vt_shape_from_config(style: seance_config::CursorStyle) -> VtCursorShape {
    match style {
        seance_config::CursorStyle::Block => VtCursorShape::Block,
        seance_config::CursorStyle::Bar => VtCursorShape::Bar,
        seance_config::CursorStyle::Underline => VtCursorShape::Underline,
    }
}

pub(crate) fn physical_window_padding(config: &Config, scale_factor: f64) -> [u16; 2] {
    let scale = |value: u16| -> u16 {
        ((f64::from(value) * scale_factor).round()).clamp(0.0, f64::from(u16::MAX)) as u16
    };
    [
        scale(config.window.padding_x),
        scale(config.window.padding_y),
    ]
}
