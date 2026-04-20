//! File watcher that forwards config/theme file changes into the winit event
//! loop as [`UserEvent`]s.
//!
//! Owns a `notify-debouncer-full` debouncer with a 100ms window (per #13) —
//! the background thread lives for the lifetime of [`ConfigWatcher`] and is
//! torn down on drop.

use std::path::{Path, PathBuf};
use std::time::Duration;

use notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{Debouncer, RecommendedCache, new_debouncer};
use winit::event_loop::EventLoopProxy;

use crate::UserEvent;

const DEBOUNCE: Duration = Duration::from_millis(100);
const THEMES_SUBDIR: &str = "themes";

/// Watches `$XDG_CONFIG_HOME/seance/` for edits to `config.toml` and anything
/// under `themes/`. Dropped when the app exits.
pub struct ConfigWatcher {
    // RAII — debouncer owns a worker thread that stops on drop.
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

impl ConfigWatcher {
    /// Start watching `config_dir`. Fails silently (returns `None`) if the
    /// directory cannot be watched — a missing config dir should never stop
    /// the terminal from launching.
    pub fn spawn(config_dir: &Path, proxy: EventLoopProxy<UserEvent>) -> Option<Self> {
        let config_file = config_dir.join(seance_config::CONFIG_FILENAME);
        let themes_dir = config_dir.join(THEMES_SUBDIR);
        let config_dir_owned = config_dir.to_path_buf();

        let handler = move |result: notify_debouncer_full::DebounceEventResult| match result {
            Ok(events) => {
                dispatch_events(&events, &config_file, &themes_dir, &proxy);
            }
            Err(errors) => {
                for e in errors {
                    log::warn!("config watcher error: {e}");
                }
            }
        };

        let mut debouncer = match new_debouncer(DEBOUNCE, None, handler) {
            Ok(d) => d,
            Err(err) => {
                log::warn!("config watcher: could not create debouncer: {err}");
                return None;
            }
        };

        // Watch the parent dir non-recursively (catches atomic-save rename +
        // delete/create of config.toml that most editors do) — the themes
        // dir is watched recursively only if it exists on disk.
        if let Err(err) = debouncer.watch(&config_dir_owned, RecursiveMode::NonRecursive) {
            log::warn!(
                "config watcher: could not watch {}: {err}",
                config_dir_owned.display()
            );
            return None;
        }
        if themes_dir.is_dir() {
            if let Err(err) = debouncer.watch(&themes_dir, RecursiveMode::Recursive) {
                log::warn!(
                    "config watcher: could not watch {}: {err}",
                    themes_dir.display()
                );
            }
        }

        log::info!("config watcher: watching {}", config_dir_owned.display());
        Some(Self {
            _debouncer: debouncer,
        })
    }
}

fn dispatch_events(
    events: &[notify_debouncer_full::DebouncedEvent],
    config_file: &Path,
    themes_dir: &Path,
    proxy: &EventLoopProxy<UserEvent>,
) {
    // Collapse a burst into at most one ConfigFileChanged + one
    // ThemeFileChanged(path) per distinct theme path.
    let mut config_changed = false;
    let mut theme_paths: Vec<PathBuf> = Vec::new();

    for ev in events {
        if !is_content_event(&ev.event.kind) {
            continue;
        }
        for path in &ev.event.paths {
            if path == config_file {
                config_changed = true;
            } else if path.starts_with(themes_dir) && !theme_paths.iter().any(|p| p == path) {
                theme_paths.push(path.clone());
            }
        }
    }

    if config_changed {
        // If the proxy send fails the event loop has already exited; the
        // next drop will clean up the watcher thread.
        let _ = proxy.send_event(UserEvent::ConfigFileChanged);
    }
    for path in theme_paths {
        let _ = proxy.send_event(UserEvent::ThemeFileChanged(path));
    }
}

/// True for events that might have changed the content of a file we care
/// about. We skip pure metadata/access events to avoid reloading on every
/// `stat()` touch.
fn is_content_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any
    )
}
