use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Modifiers, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

use seance_config::{Config, ConfigDiff, MacosOptionAsAlt};
use seance_input::{InputHandler, OptionAsAlt, VtInput};
use seance_render::{RenderInputs, RendererConfig, TerminalRenderer};
use seance_vt::{
    CursorShape as VtCursorShape, FrameSource, LibGhosttyFrameSource, Terminal, TerminalModes,
};

mod command;
mod keybinds;
mod platform;
mod watcher;

use command::AppCommand;
use keybinds::Keybinds;
use watcher::ConfigWatcher;

/// Events forwarded from the config watcher into the winit event loop. Using
/// `EventLoopProxy` keeps reloads single-threaded with rendering — no torn
/// reads of `Config` mid-frame.
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// `config.toml` at `$XDG_CONFIG_HOME/seance/` changed.
    ConfigFileChanged,
    /// A file under `$XDG_CONFIG_HOME/seance/themes/` changed.
    ThemeFileChanged(PathBuf),
}

const FONT_SIZE_MIN: f32 = 6.0;
const FONT_SIZE_MAX: f32 = 72.0;
const POLL_INTERVAL: Duration = Duration::from_millis(4);
const MULTI_CLICK_WINDOW: Duration = Duration::from_millis(500);
// Half-period of the cursor blink cycle; on + off = 1 s. The tick rides on
// POLL_INTERVAL wakeups — once M2 #24 lands we should drive it from the
// deadline scheduler instead.
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);

fn initial_window_size_from_env() -> Option<LogicalSize<u32>> {
    let value = std::env::var("SEANCE_INITIAL_WINDOW_SIZE").ok()?;
    let (width, height) = value.split_once(',').or_else(|| value.split_once('x'))?;
    let width = width.parse().ok()?;
    let height = height.parse().ok()?;
    Some(LogicalSize::new(width, height))
}

fn vt_shape_from_config(style: seance_config::CursorStyle) -> VtCursorShape {
    match style {
        seance_config::CursorStyle::Block => VtCursorShape::Block,
        seance_config::CursorStyle::Bar => VtCursorShape::Bar,
        seance_config::CursorStyle::Underline => VtCursorShape::Underline,
    }
}

fn physical_window_padding(config: &Config, scale_factor: f64) -> [u16; 2] {
    let scale = |value: u16| -> u16 {
        ((f64::from(value) * scale_factor).round()).clamp(0.0, f64::from(u16::MAX)) as u16
    };
    [
        scale(config.window.padding_x),
        scale(config.window.padding_y),
    ]
}

struct MouseState {
    cursor_pos: PhysicalPosition<f64>,
    is_down: bool,
    click_count: u8,
    last_click_time: Instant,
    last_click_pos: (u16, u16),
}

impl Default for MouseState {
    fn default() -> Self {
        Self {
            cursor_pos: PhysicalPosition::new(0.0, 0.0),
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
        let fast = now.duration_since(self.last_click_time) < MULTI_CLICK_WINDOW;
        let same_spot = (col, row) == self.last_click_pos;
        self.click_count = if fast && same_spot {
            (self.click_count % 3) + 1
        } else {
            1
        };
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
    keybinds: Keybinds,
    render_inputs: RenderInputs,
    modifiers: Modifiers,
    cell_size: [f32; 2],
    config: Config,
    font_size: f32,
    content_dirty: bool,
    occluded: bool,
    mouse: MouseState,
    proxy: EventLoopProxy<UserEvent>,
    watcher: Option<ConfigWatcher>,
    blink_on: bool,
    last_blink_edge: Instant,
    // `None` until the VT has reported a shape via DECSCUSR; then the
    // config's `cursor.style` acts as the fallback when the VT has no
    // opinion (e.g. FFI error path in `LibGhosttyFrameSource::cursor`).
    last_vt_cursor_shape: Option<VtCursorShape>,
}

impl App {
    fn new(config: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        let font_size = config.font.size;
        let render_inputs = RenderInputs {
            cursor_shape: config.cursor.style.into(),
            ..RenderInputs::default()
        };
        let mut input = InputHandler::new();
        input.set_option_as_alt(option_as_alt_from_config(config.input.macos_option_as_alt));
        Self {
            window: None,
            renderer: None,
            terminal: None,
            input,
            keybinds: Keybinds::new(),
            render_inputs,
            modifiers: Modifiers::default(),
            cell_size: [0.0, 0.0],
            config,
            font_size,
            content_dirty: true,
            occluded: false,
            mouse: MouseState::default(),
            proxy,
            watcher: None,
            blink_on: true,
            last_blink_edge: Instant::now(),
            last_vt_cursor_shape: None,
        }
    }

    fn request_redraw(&self) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn mark_dirty(&mut self) {
        self.content_dirty = true;
        self.request_redraw();
    }

    fn poll_pty(&mut self) {
        let Some(term) = &mut self.terminal else {
            return;
        };
        if term.poll() {
            self.content_dirty = true;
        }
        if !term.is_alive() {
            self.terminal = None;
        }
    }

    fn draw(&mut self) {
        if self.occluded || self.terminal.is_none() {
            return;
        }
        if self.content_dirty
            && let (Some(r), Some(t)) = (&mut self.renderer, &mut self.terminal)
        {
            self.content_dirty = false;
            let mut source = LibGhosttyFrameSource::new(t);
            // Cache the VT's DECSCUSR-tracked shape (if any) before the
            // renderer consumes the source — mode changes in neovim arrive
            // as PTY bytes that set `content_dirty`, so this branch runs
            // on every mode transition.
            self.last_vt_cursor_shape = source.cursor().shape;
            r.update_frame(&mut source);
        }
        // Prefer the VT-reported shape; fall back to the user's configured
        // default when the VT has no opinion. Refreshed every frame so that
        // hot-reload of `cursor.style` is picked up without extra wiring.
        self.render_inputs.cursor_shape = self
            .last_vt_cursor_shape
            .map(Into::into)
            .unwrap_or_else(|| self.config.cursor.style.into());
        self.render_inputs.vt_cursor_visible = !self.config.cursor.blink || self.blink_on;
        if let Some(r) = &mut self.renderer {
            r.render(&self.render_inputs);
        }
    }

    fn tick_blink(&mut self) {
        if !self.config.cursor.blink {
            if !self.blink_on {
                self.blink_on = true;
                self.mark_dirty();
            }
            return;
        }
        if self.last_blink_edge.elapsed() >= BLINK_HALF_PERIOD {
            self.blink_on = !self.blink_on;
            self.last_blink_edge = Instant::now();
            self.mark_dirty();
        }
    }

    /// Resize the surface and reflow the VT grid.
    fn reflow(&mut self, pixel_size: PhysicalSize<u32>) {
        let Some(r) = &mut self.renderer else {
            return;
        };
        r.resize_surface(pixel_size.width, pixel_size.height);
        self.cell_size = r.cell_size();
        let (cols, rows) = r.grid_size();
        if let Some(term) = &mut self.terminal {
            term.resize(
                cols,
                rows,
                pixel_size.width as u16,
                pixel_size.height as u16,
            );
        }
        self.mark_dirty();
    }

    fn apply_font_size(&mut self) {
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            r.set_font_size(self.font_size);
            self.reflow(w.inner_size());
        }
    }

    fn apply_font_metrics(&mut self, font_size_changed: bool, adjust_cell_height_changed: bool) {
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            if adjust_cell_height_changed {
                r.set_adjust_cell_height(self.config.font.adjust_cell_height.as_deref());
            }
            if font_size_changed {
                r.set_font_size(self.font_size);
            }
            self.reflow(w.inner_size());
        }
    }

    fn apply_scale_factor(&mut self, scale_factor: f64) {
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            r.set_scale(scale_factor);
            r.set_window_padding(physical_window_padding(&self.config, scale_factor));
            self.reflow(w.inner_size());
        }
    }

    fn apply_terminal_theme(&mut self, theme: &seance_config::Theme) {
        if let Some(term) = &mut self.terminal {
            term.set_theme_colors(
                theme.fg,
                [theme.bg[0], theme.bg[1], theme.bg[2]],
                [theme.cursor[0], theme.cursor[1], theme.cursor[2]],
                theme.palette,
            );
        }
    }

    /// Re-resolve the currently-configured theme and push it to the renderer.
    /// Bad theme files keep the previous theme live (#13).
    fn reload_theme(&mut self) {
        if self.renderer.is_none() {
            return;
        }
        let spec = seance_config::theme::ThemeSpec::parse(
            self.config
                .theme
                .as_deref()
                .unwrap_or(seance_config::theme::resolve::DEFAULT_THEME_NAME),
        );
        let theme = match seance_config::theme::try_load(&spec) {
            Ok(t) => t,
            Err(err) => {
                log::warn!("theme reload skipped: {err}");
                return;
            }
        };
        if let Some(r) = &mut self.renderer {
            r.set_theme(theme.clone());
        }
        self.apply_terminal_theme(&theme);
        self.mark_dirty();
    }

    /// Re-parse `config.toml` and apply whatever actually changed. A bad
    /// TOML parse is logged and the running config is left untouched.
    fn reload_config(&mut self) {
        let Some(path) = seance_config::config_file_path() else {
            return;
        };
        let new_config = match seance_config::try_load_from(&path) {
            Ok(c) => c,
            Err(err) => {
                log::warn!("config reload skipped: {err}");
                return;
            }
        };
        let old_config = self.config.clone();
        let diff = ConfigDiff::between(&old_config, &new_config);
        if diff.is_empty() {
            self.config = new_config;
            return;
        }

        log::info!("config reloaded: {diff:?}");
        self.config = new_config;

        if let Some(r) = &mut self.renderer {
            if old_config.font.min_contrast != self.config.font.min_contrast {
                r.set_min_contrast(self.config.font.min_contrast);
            }
            if old_config.window.background_opacity != self.config.window.background_opacity {
                r.set_background_opacity(self.config.window.background_opacity);
            }
        }

        if diff.font_size_changed {
            self.font_size = self.config.font.size;
        }
        if diff.font_size_changed || diff.font_adjust_cell_height_changed {
            self.apply_font_metrics(diff.font_size_changed, diff.font_adjust_cell_height_changed);
        }
        if diff.font_family_changed {
            log::info!("font.family change takes effect on restart (live swap not yet supported)");
        }
        if diff.theme_changed {
            self.reload_theme();
        }
        if diff.window_padding_changed {
            self.apply_window_padding();
        }
        if diff.input_changed {
            let mode = option_as_alt_from_config(self.config.input.macos_option_as_alt);
            self.input.set_option_as_alt(mode);
            if let Some(w) = &self.window {
                platform::set_option_as_alt(w, mode);
            }
        }
        if old_config.cursor.style != self.config.cursor.style
            && let Some(t) = &mut self.terminal
        {
            t.set_cursor_shape(vt_shape_from_config(self.config.cursor.style));
        }
        if diff.repaint_only {
            self.mark_dirty();
        }
    }

    /// Push the configured window padding to the renderer and reflow the PTY.
    /// `grid_size()` shrinks when padding grows, so a reflow is required to
    /// keep the shell's SIGWINCH in sync.
    fn apply_window_padding(&mut self) {
        if let (Some(r), Some(w)) = (&mut self.renderer, &self.window) {
            r.set_window_padding(physical_window_padding(&self.config, w.scale_factor()));
            self.reflow(w.inner_size());
        }
    }

    /// A file under `themes/` changed on disk. Only re-resolve if it's the
    /// theme actually in use (either a named override in the user dir or an
    /// absolute-path spec pointing at that file).
    fn on_theme_file_changed(&mut self, path: &std::path::Path) {
        let active = self
            .config
            .theme
            .as_deref()
            .unwrap_or(seance_config::theme::resolve::DEFAULT_THEME_NAME);
        let spec = seance_config::theme::ThemeSpec::parse(active);
        let matches = match &spec {
            seance_config::theme::ThemeSpec::Named(name)
            | seance_config::theme::ThemeSpec::LightDark { dark: name, .. } => {
                seance_config::config_dir()
                    .map(|d| d.join("themes").join(name))
                    .is_some_and(|p| p == path)
            }
            seance_config::theme::ThemeSpec::Path(p) => p == path,
        };
        if matches {
            self.reload_theme();
        }
    }

    fn terminal_modes(&self) -> TerminalModes {
        self.terminal
            .as_ref()
            .map(Terminal::modes)
            .unwrap_or_default()
    }

    fn has_selection(&self) -> bool {
        self.terminal.as_ref().is_some_and(Terminal::has_selection)
    }

    fn clear_selection(&mut self) {
        if let Some(term) = &mut self.terminal {
            term.clear_selection();
        }
        self.render_inputs.selection = None;
    }

    fn sync_selection_to_overlay(&mut self) {
        self.render_inputs.selection = self.terminal.as_ref().and_then(Terminal::selection_range);
    }

    fn copy_selection_to_clipboard(&mut self) {
        let Some(text) = self.terminal.as_mut().and_then(Terminal::selection_text) else {
            return;
        };
        if text.is_empty() {
            return;
        }
        if let Ok(mut cb) = arboard::Clipboard::new() {
            let _ = cb.set_text(text);
        }
    }

    fn on_keyboard_input(&mut self, event_loop: &ActiveEventLoop, event: &winit::event::KeyEvent) {
        if let Some(cmd) = self.keybinds.match_event(event, &self.modifiers) {
            let preserves_selection = matches!(cmd, AppCommand::Copy | AppCommand::SelectAll);
            if !preserves_selection && self.has_selection() {
                self.clear_selection();
            }
            self.execute_app_command(event_loop, cmd);
            self.mark_dirty();
            return;
        }

        let input = self
            .input
            .handle_key(event, &self.modifiers, self.terminal_modes());

        if event.state == ElementState::Pressed
            && !matches!(input, VtInput::Ignore)
            && self.has_selection()
        {
            self.clear_selection();
        }

        if let VtInput::Write(bytes) = input
            && let Some(term) = &self.terminal
        {
            term.write(&bytes);
        }
        self.mark_dirty();
    }

    fn execute_app_command(&mut self, event_loop: &ActiveEventLoop, cmd: AppCommand) {
        match cmd {
            AppCommand::Quit | AppCommand::CloseWindow => event_loop.exit(),
            AppCommand::Copy => {
                self.copy_selection_to_clipboard();
                self.clear_selection();
            }
            AppCommand::Paste => self.paste_from_clipboard(),
            AppCommand::SelectAll => {
                if let Some(term) = &mut self.terminal {
                    term.select_all();
                }
                self.sync_selection_to_overlay();
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

    fn paste_from_clipboard(&mut self) {
        let Ok(mut cb) = arboard::Clipboard::new() else {
            return;
        };
        let Ok(text) = cb.get_text() else {
            return;
        };
        let Some(term) = &self.terminal else {
            return;
        };
        let bracketed = term.modes().bracketed_paste;
        if bracketed {
            term.write(b"\x1b[200~");
        }
        term.write(text.as_bytes());
        if bracketed {
            term.write(b"\x1b[201~");
        }
    }

    fn on_mouse_wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        let lines = match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y as i32,
            winit::event::MouseScrollDelta::PixelDelta(pos) => {
                let ch = self.cell_size[1].max(1.0);
                (pos.y / f64::from(ch)) as i32
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
        self.mark_dirty();
    }

    fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        self.mouse.cursor_pos = position;
        if !self.mouse.is_down {
            return;
        }
        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        let (col, row) = r.pixel_to_grid(position.x, position.y);
        if let Some(term) = &mut self.terminal {
            term.update_selection(col, row);
        }
        self.sync_selection_to_overlay();
        self.mark_dirty();
    }

    fn on_mouse_input(&mut self, state: ElementState, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        match state {
            ElementState::Pressed => self.handle_mouse_press(),
            ElementState::Released => {
                self.mouse.is_down = false;
                self.copy_selection_to_clipboard();
            }
        }
    }

    fn handle_mouse_press(&mut self) {
        if self.modifiers.state().super_key()
            && let Some(window) = self.window.as_ref()
        {
            let _ = window.drag_window();
            return;
        }

        let Some(r) = self.renderer.as_ref() else {
            return;
        };
        let (col, row) = r.pixel_to_grid(self.mouse.cursor_pos.x, self.mouse.cursor_pos.y);
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
        self.mouse.is_down = true;
        self.mark_dirty();
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
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

        #[cfg(target_os = "macos")]
        if self.config.window.decoration {
            platform::configure_window(&window);
        }
        platform::set_option_as_alt(
            &window,
            option_as_alt_from_config(self.config.input.macos_option_as_alt),
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

        self.cell_size = renderer.cell_size();
        let (cols, rows) = renderer.grid_size();
        self.renderer = Some(renderer);
        self.window = Some(window);

        let mut term = Terminal::spawn(cols, rows, size.width as u16, size.height as u16)
            .expect("failed to spawn terminal");
        // Seed the VT's DECSCUSR state with the user's configured shape so
        // the bash prompt doesn't inherit ghostty's hardcoded `.block`
        // default. App-level DECSCUSR emissions (e.g. neovim mode changes)
        // still override on subsequent frames.
        term.set_cursor_shape(vt_shape_from_config(self.config.cursor.style));
        self.terminal = Some(term);
        self.apply_terminal_theme(&theme);

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
                self.reflow(size);
                self.draw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.apply_scale_factor(scale_factor);
                self.draw();
            }
            WindowEvent::ModifiersChanged(mods) => self.modifiers = mods,
            WindowEvent::KeyboardInput { event, .. } => self.on_keyboard_input(event_loop, &event),
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_input(state, button),
            WindowEvent::Occluded(is_occluded) => {
                self.occluded = is_occluded;
                if !is_occluded {
                    self.mark_dirty();
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.poll_pty();
        if self.terminal.is_none() {
            event_loop.exit();
            return;
        }
        self.tick_blink();
        if self.content_dirty && !self.occluded {
            self.request_redraw();
        }
        event_loop.set_control_flow(ControlFlow::wait_duration(POLL_INTERVAL));
    }
}

fn option_as_alt_from_config(cfg: MacosOptionAsAlt) -> OptionAsAlt {
    match cfg {
        MacosOptionAsAlt::None => OptionAsAlt::None,
        MacosOptionAsAlt::Left => OptionAsAlt::Left,
        MacosOptionAsAlt::Right => OptionAsAlt::Right,
        MacosOptionAsAlt::Both => OptionAsAlt::Both,
    }
}

fn main() {
    env_logger::init();
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Regular);
    }
    let event_loop = builder.build().expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let config = seance_config::load();
    let mut app = App::new(config, proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
