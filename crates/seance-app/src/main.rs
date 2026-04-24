use std::path::PathBuf;

use winit::event_loop::EventLoop;

mod app;
mod command;
mod keybinds;
mod mouse;
mod platform;
mod reload;
mod watcher;
mod window_state;

use app::App;

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
