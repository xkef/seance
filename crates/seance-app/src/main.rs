use std::path::PathBuf;

use seance_mux::MuxEvent;
use winit::event_loop::EventLoop;

mod app;
mod apply;
mod command;
mod events;
mod keybinds;
mod mouse;
mod platform;
mod reload;
mod surface_state;
mod watcher;

use app::App;

/// Events forwarded from background threads into the winit event loop.
/// Using `EventLoopProxy` keeps every off-thread signal — config reloads,
/// pane frame publication, child exit — funnelled onto the single UI thread
/// that owns the renderer, so there are no torn reads of `Config` or races
/// against frame state.
#[derive(Debug, Clone)]
pub enum UserEvent {
    /// `config.toml` at `$XDG_CONFIG_HOME/seance/` changed.
    ConfigFileChanged,
    /// A file under `$XDG_CONFIG_HOME/seance/themes/` changed.
    ThemeFileChanged(PathBuf),
    /// The local mux published a pane update or reported child exit.
    Mux(MuxEvent),
}

fn main() {
    env_logger::init();
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    platform::configure_event_loop(&mut builder);
    let event_loop = builder.build().expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let config = seance_config::load();
    let mut app = App::new(config, proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
