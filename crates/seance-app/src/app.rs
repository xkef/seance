//! `App` — the top-level winit `ApplicationHandler`.
//!
//! Owns process-lifetime state (config, input handler, config watcher) and
//! a single `window_state: Option<WindowState>` for everything that exists
//! only while a window is up.
//!
//! Peer modules:
//! - `events.rs` — winit event handlers (keyboard, mouse).
//! - `apply.rs`  — propagate settings changes (font, scale, padding) into
//!   the renderer and reflow the PTY.
//! - `reload.rs` — hot-reload config / theme files.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Window, WindowId};

use seance_config::Config;
use seance_input::InputHandler;
use seance_render::{RenderInputs, RendererConfig, TerminalRenderer};
use seance_vt::{CursorShape as VtCursorShape, FrameSource, LibGhosttyFrameSource, Terminal};

use crate::UserEvent;
use crate::io::spawn_pty_reader;
use crate::keybinds::Keybinds;
use crate::platform;
use crate::watcher::ConfigWatcher;
use crate::window_state::WindowState;

/// Half-period of the cursor blink cycle; on + off = 1 s. Drives the
/// deadline scheduler — when blink is enabled, the next animation wake
/// is `last_blink_edge + BLINK_HALF_PERIOD`.
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);

pub(crate) struct App {
    pub(crate) window_state: Option<WindowState>,
    pub(crate) input: InputHandler,
    pub(crate) keybinds: Keybinds,
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
            let selection = ws.selection_range();
            let mut source = LibGhosttyFrameSource::new(&mut ws.terminal, selection);
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

    /// Advance the cursor blink state if we have crossed an edge. Called
    /// from `about_to_wait` after the deadline-scheduled wake fires.
    fn step_blink(&mut self) {
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

    /// Earliest instant at which any animation source needs the next
    /// wake. `None` means the terminal is idle — `about_to_wait` will
    /// drop into `ControlFlow::Wait` and the OS suspends us until either
    /// a window event arrives or the IO thread signals via the proxy.
    fn next_animation_deadline(&self) -> Option<Instant> {
        let ws = self.window_state.as_ref()?;
        // Occluded windows skip rendering anyway, so don't bother
        // running the blink cycle while the window is hidden.
        if ws.occluded {
            return None;
        }
        if self.config.cursor.blink {
            Some(ws.last_blink_edge + BLINK_HALF_PERIOD)
        } else {
            None
        }
    }
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

        // Move the PTY reader onto a dedicated thread; from here the UI
        // wakes only when the IO thread forwards `PtyData` / `PtyExited`
        // through the proxy or when an animation deadline fires.
        let reader = term
            .take_reader()
            .expect("Terminal::spawn must hand out the PTY reader");
        spawn_pty_reader(reader, self.proxy.clone());

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

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::ConfigFileChanged => self.reload_config(),
            UserEvent::ThemeFileChanged(path) => self.on_theme_file_changed(&path),
            UserEvent::PtyData(bytes) => {
                if let Some(ws) = self.ws_mut() {
                    ws.feed_pty(&bytes);
                }
            }
            UserEvent::PtyExited => {
                self.window_state = None;
                event_loop.exit();
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
        if self.window_state.is_none() {
            event_loop.exit();
            return;
        }
        self.step_blink();
        if let Some(ws) = self.window_state.as_ref()
            && ws.content_dirty
            && !ws.occluded
        {
            ws.request_redraw();
        }
        // Deadline-scheduled redraw: sleep until the next animation
        // edge, or fully `Wait` when nothing is animating. PTY output
        // wakes us out-of-band via `UserEvent::PtyData` from the reader
        // thread, so an idle terminal really does park the event loop.
        match self.next_animation_deadline() {
            Some(deadline) => event_loop.set_control_flow(ControlFlow::WaitUntil(deadline)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
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
