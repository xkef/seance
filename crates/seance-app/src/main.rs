use std::path::PathBuf;

use seance_mux::MuxEvent;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
use winit::event_loop::EventLoop;

mod app;
mod apply;
mod command;
mod events;
mod keybinds;
mod link_open;
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
    /// The mux layer has updates ready to drain.
    Mux(MuxEvent),
}

/// `$HOME`-rooted log directory for the rolling file appender.
///
/// macOS uses `~/Library/Logs/seance/`, the convention for user-scoped
/// logs even for unbundled CLIs. Linux follows XDG state with the
/// usual `~/.local/state/seance/` fallback.
fn log_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    #[cfg(target_os = "macos")]
    {
        Some(PathBuf::from(home).join("Library/Logs/seance"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            Some(PathBuf::from(xdg).join("seance"))
        } else {
            Some(PathBuf::from(home).join(".local/state/seance"))
        }
    }
}

/// Install the global `tracing` subscriber with a stdout layer plus an
/// optional non-blocking daily-rolling file layer. The returned guard
/// must outlive every emitted event; main holds it for the lifetime of
/// the event loop so the appender flushes on shutdown.
fn init_tracing() -> Option<WorkerGuard> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "seance=info,wgpu=warn,wgpu_core=warn,wgpu_hal=warn,\
             winit=warn,cosmic_text=warn,naga=warn,notify=warn",
        )
    });
    let stdout_layer = fmt::layer().with_writer(std::io::stdout).with_target(false);
    let (file_layer, guard) = match log_dir() {
        Some(dir) if std::fs::create_dir_all(&dir).is_ok() => {
            let appender = rolling::daily(&dir, "seance.log");
            let (nb, g) = tracing_appender::non_blocking(appender);
            (Some(fmt::layer().with_writer(nb).with_ansi(false)), Some(g))
        }
        _ => (None, None),
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();
    guard
}

fn main() {
    let _log_guard = init_tracing();
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    platform::configure_event_loop(&mut builder);
    let event_loop = builder.build().expect("failed to create event loop");
    let proxy = event_loop.create_proxy();
    let config = seance_config::load();
    let mut app = App::new(config, proxy);
    event_loop.run_app(&mut app).expect("event loop failed");
}
