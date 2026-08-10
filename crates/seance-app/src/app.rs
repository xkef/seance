//! `App` — the top-level winit `ApplicationHandler`.
//!
//! Owns process-lifetime state (config, input handler, config watcher) and
//! a single `surface: Option<SurfaceState>` for everything that exists only
//! while an OS window is up.
//!
//! Peer modules:
//! - `events.rs` — winit event handlers (keyboard, mouse).
//! - `apply.rs`  — propagate settings changes (font, scale, padding) into
//!   the renderer and reflow the PTY.
//! - `reload.rs` — hot-reload config / theme files.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::window::{Window, WindowId};

use seance_config::Config;
use seance_input::InputHandler;
use seance_mux_client::{
    CursorShape as MuxCursorShape, LinkDetector, LinkModifiers, MuxClient, MuxEvent,
    PaneSpawnOptions, ProtocolDomain,
};
use seance_render::{RenderInputs, RendererConfig, TerminalRenderer};

use crate::UserEvent;
use crate::keybinds::Keybinds;
use crate::platform;
use crate::surface_state::SurfaceState;
use crate::watcher::ConfigWatcher;

/// Half-period of the cursor blink cycle; on + off = 1 s. Drives the
/// deadline scheduler — when blink is enabled, the next animation wake
/// is `last_blink_edge + BLINK_HALF_PERIOD`.
const BLINK_HALF_PERIOD: Duration = Duration::from_millis(500);

pub(crate) struct App {
    pub(crate) surface: Option<SurfaceState>,
    pub(crate) input: InputHandler,
    pub(crate) keybinds: Keybinds,
    pub(crate) config: Config,
    pub(crate) font_size: f32,
    proxy: EventLoopProxy<UserEvent>,
    watcher: Option<ConfigWatcher>,
    /// Handle to the in-process mux-server thread that owns LocalDomain.
    /// Dropping it (on `App` drop / shutdown) closes the InProcessTransport
    /// which causes `serve` to return and the thread to exit cleanly.
    _server_thread: Option<JoinHandle<()>>,
}

impl App {
    pub(crate) fn new(config: Config, proxy: EventLoopProxy<UserEvent>) -> Self {
        let font_size = config.font.size;
        let mut input = InputHandler::new();
        input.set_option_as_alt(platform::option_as_alt_from_config(
            config.input.macos_option_as_alt,
        ));
        Self {
            surface: None,
            input,
            keybinds: Keybinds::from_config(&config.keybind),
            config,
            font_size,
            proxy,
            watcher: None,
            _server_thread: None,
        }
    }

    /// Shortcut — most methods run only while a surface is up.
    pub(crate) fn surface_mut(&mut self) -> Option<&mut SurfaceState> {
        self.surface.as_mut()
    }

    pub(crate) fn mark_dirty(&mut self) {
        if let Some(surface) = self.surface_mut() {
            surface.mark_dirty();
        }
    }

    fn draw(&mut self) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if surface.occluded {
            return;
        }
        if surface.content_dirty {
            surface.content_dirty = false;
            surface.last_vt_cursor_shape = surface
                .mux
                .pane_view(surface.active_pane)
                .and_then(|view| view.cursor_shape());
            if let Some(mut source) = surface
                .mux
                .pane_view(surface.active_pane)
                .and_then(|view| view.frame_source())
            {
                surface.renderer.update_frame(&mut source);
            }
        }
        let selection = surface.selection_range();
        let hovered_link = surface.hovered_link_range();
        surface.render_inputs.selection = selection;
        surface.render_inputs.hovered_link = hovered_link;
        // Prefer the VT-reported shape; fall back to the user's configured
        // default when the VT has no opinion. Refreshed every frame so that
        // hot-reload of `cursor.style` is picked up without extra wiring.
        surface.render_inputs.cursor_shape = surface
            .last_vt_cursor_shape
            .map(Into::into)
            .unwrap_or_else(|| self.config.cursor.style.into());
        surface.render_inputs.vt_cursor_visible = !self.config.cursor.blink || surface.blink_on;
        let rendered_generation = surface
            .mux
            .pane_view(surface.active_pane)
            .and_then(|view| view.generation());
        if surface.renderer.render(&surface.render_inputs)
            && let Some(generation) = rendered_generation
        {
            surface.ack_presented(generation);
        }
    }

    /// Advance the cursor blink state if we have crossed an edge. Called
    /// from `about_to_wait` after the deadline-scheduled wake fires.
    fn step_blink(&mut self) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if !self.config.cursor.blink {
            if !surface.blink_on {
                surface.blink_on = true;
                surface.mark_dirty();
            }
            return;
        }
        if surface.last_blink_edge.elapsed() >= BLINK_HALF_PERIOD {
            surface.blink_on = !surface.blink_on;
            surface.last_blink_edge = Instant::now();
            surface.mark_dirty();
        }
    }

    /// Clear a flashed selection once its deadline has passed.
    fn step_selection_flash(&mut self) {
        let Some(surface) = self.surface.as_mut() else {
            return;
        };
        if let Some(deadline) = surface.selection_dismiss_at
            && Instant::now() >= deadline
        {
            surface.selection_dismiss_at = None;
            if surface.has_selection() {
                surface.clear_selection();
            }
            surface.mark_dirty();
        }
    }

    /// Earliest instant at which any animation source needs the next
    /// wake. `None` means the terminal is idle — `about_to_wait` will
    /// drop into `ControlFlow::Wait` and the OS suspends us until either
    /// a window event arrives or the mux signals via the proxy.
    fn next_animation_deadline(&self) -> Option<Instant> {
        let surface = self.surface.as_ref()?;
        // Occluded windows skip rendering anyway, so don't bother
        // running the blink cycle while the window is hidden.
        if surface.occluded {
            return None;
        }
        let blink = self
            .config
            .cursor
            .blink
            .then(|| surface.last_blink_edge + BLINK_HALF_PERIOD);
        match (blink, surface.selection_dismiss_at) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(d), None) | (None, Some(d)) => Some(d),
            (None, None) => None,
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_some() {
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
        tracing::info!(
            width = size.width,
            height = size.height,
            scale = window.scale_factor(),
            "window created",
        );
        let theme = seance_config::load_theme(self.config.theme.as_deref());
        let renderer_config = RendererConfig {
            width: size.width,
            height: size.height,
            scale: window.scale_factor(),
            font_family: self.config.font.family.clone(),
            font_size: self.font_size,
            adjust_cell_height: self.config.font.adjust_cell_height.clone(),
            adjust_cell_width: self.config.font.adjust_cell_width.clone(),
            font_features: self.config.font.features.clone(),
            min_contrast: self.config.font.min_contrast,
            window_padding: physical_window_padding(&self.config, window.scale_factor()),
            window_padding_balance: self.config.window.padding_balance,
            background_opacity: self.config.window.background_opacity,
            theme: theme.clone(),
        };

        let renderer = pollster::block_on(TerminalRenderer::new(window.clone(), renderer_config))
            .expect("failed to create renderer");
        platform::configure_metal_layer(&window);

        let (cols, rows) = renderer.grid_size();
        let proxy = self.proxy.clone();
        let link_detector = link_detector_from_config(&self.config.links);
        // Local mode runs the wire protocol end-to-end: LocalDomain lives on
        // a server thread (seance-mux-server), the frontend talks to it
        // through ProtocolDomain<InProcessTransport>. Same shape as a future
        // Unix/SSH/TLS client (M12) — the only thing that swaps is the
        // Transport.
        let (client_transport, server_thread) = seance_mux_server::spawn_local_server(move || {
            let _ = proxy.send_event(UserEvent::Mux(MuxEvent::Wake));
        });
        self._server_thread = Some(server_thread);
        let mut mux =
            MuxClient::with_link_detector(ProtocolDomain::new(client_transport), link_detector);
        let active_pane = mux
            .spawn_pane(PaneSpawnOptions {
                cols,
                rows,
                pixel_width: size.width as u16,
                pixel_height: size.height as u16,
                initial_cursor_shape: mux_shape_from_config(self.config.cursor.style),
                max_scrollback: self.config.scrollback.limit as usize,
            })
            .expect("failed to spawn local pane");
        tracing::info!(pane = ?active_pane, cols, rows, "pane spawned");

        let render_inputs = RenderInputs {
            cursor_shape: self.config.cursor.style.into(),
            ..RenderInputs::default()
        };
        let mut surface = SurfaceState::new(window, renderer, mux, active_pane, render_inputs);
        self.apply_terminal_theme_to(&mut surface, &theme);
        self.surface = Some(surface);

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
            UserEvent::Mux(MuxEvent::Wake) => {
                let _span = tracing::debug_span!("mux::refresh").entered();
                let mut should_exit = false;
                let read_policy = self.config.clipboard.read;
                let write_policy = self.config.clipboard.write;
                if let Some(surface) = self.surface_mut() {
                    match surface.refresh_updates() {
                        Ok(refresh) => {
                            let frame_dirty = refresh.frame_dirty;
                            let image_events = refresh.image_events;
                            let exited = refresh.exited;
                            let clipboard_requests = refresh.clipboard_requests;
                            for image_event in &image_events {
                                surface.renderer.apply_image_cache_event(image_event);
                            }
                            for err in refresh.errors {
                                tracing::warn!("pane error: {err}");
                            }
                            for (_pane, request) in clipboard_requests {
                                surface.handle_clipboard_request(
                                    request,
                                    read_policy,
                                    write_policy,
                                );
                            }
                            if frame_dirty || !image_events.is_empty() {
                                surface.mark_dirty();
                            }
                            if frame_dirty {
                                surface.refresh_hovered_link();
                            }
                            should_exit = exited.contains(&surface.active_pane);
                        }
                        Err(err) => tracing::warn!("mux refresh failed: {err}"),
                    }
                }
                if should_exit {
                    tracing::info!("pane closed; shutting down");
                    self.surface = None;
                    event_loop.exit();
                }
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
            WindowEvent::CloseRequested => {
                tracing::info!("close requested; shutting down");
                self.surface = None;
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(surface) = self.surface_mut() {
                    surface.reflow(size);
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.apply_scale_factor(scale_factor);
            }
            WindowEvent::ModifiersChanged(mods) => {
                if let Some(surface) = self.surface_mut() {
                    surface.modifiers = mods;
                    surface.refresh_hovered_link();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.on_keyboard_input(event_loop, &event),
            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),
            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::MouseInput { state, button, .. } => self.on_mouse_input(state, button),
            WindowEvent::Occluded(is_occluded) => {
                if let Some(surface) = self.surface_mut() {
                    surface.occluded = is_occluded;
                    if !is_occluded {
                        surface.mark_dirty();
                    }
                }
            }
            WindowEvent::RedrawRequested => self.draw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.surface.is_none() {
            event_loop.exit();
            return;
        }
        self.step_blink();
        self.step_selection_flash();
        if let Some(surface) = self.surface.as_ref()
            && surface.content_dirty
            && !surface.occluded
        {
            surface.request_redraw();
        }
        // Deadline-scheduled redraw: sleep until the next animation
        // edge, or fully `Wait` when nothing is animating. PTY output
        // wakes us out-of-band via `UserEvent::Mux(FrameDirty)`, so an idle
        // terminal really does park the event loop.
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

pub(crate) fn mux_shape_from_config(style: seance_config::CursorStyle) -> MuxCursorShape {
    match style {
        seance_config::CursorStyle::Block => MuxCursorShape::Block,
        seance_config::CursorStyle::Bar => MuxCursorShape::Bar,
        seance_config::CursorStyle::Underline => MuxCursorShape::Underline,
    }
}

pub(crate) fn link_detector_from_config(config: &seance_config::LinksConfig) -> LinkDetector {
    let modifiers = link_modifiers_from_config(config.modifiers);
    LinkDetector::from_options(modifiers, config.url, config.paths).unwrap_or_else(|err| {
        tracing::warn!("failed to compile link detector: {err}");
        LinkDetector::from_options(modifiers, false, false)
            .expect("disabled link detector should compile")
    })
}

fn link_modifiers_from_config(config: seance_config::LinkModifiersConfig) -> LinkModifiers {
    match config {
        seance_config::LinkModifiersConfig::SuperShift => LinkModifiers {
            super_key: true,
            shift: true,
            ..LinkModifiers::default()
        },
        seance_config::LinkModifiersConfig::CtrlShift => LinkModifiers {
            ctrl: true,
            shift: true,
            ..LinkModifiers::default()
        },
        seance_config::LinkModifiersConfig::Super => LinkModifiers {
            super_key: true,
            ..LinkModifiers::default()
        },
        seance_config::LinkModifiersConfig::Ctrl => LinkModifiers {
            ctrl: true,
            ..LinkModifiers::default()
        },
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
