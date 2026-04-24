//! Hot-reload handlers: config file and theme file changes. Called from the
//! `UserEvent` branch of the winit event loop.

use seance_config::ConfigDiff;

use crate::app::{App, vt_shape_from_config};
use crate::platform;
use crate::window_state::WindowState;

impl App {
    /// Push theme colors into the live VT. Takes `&mut WindowState` so the
    /// caller (including `resumed`, where `self.window_state` isn't wired
    /// yet) can apply the theme before publishing the WindowState.
    pub(crate) fn apply_terminal_theme_to(
        &self,
        ws: &mut WindowState,
        theme: &seance_config::Theme,
    ) {
        ws.terminal.set_theme_colors(
            theme.fg,
            [theme.bg[0], theme.bg[1], theme.bg[2]],
            [theme.cursor[0], theme.cursor[1], theme.cursor[2]],
            theme.palette,
        );
    }

    /// Re-resolve the currently-configured theme and push it to the renderer.
    /// Bad theme files keep the previous theme live (#13).
    pub(crate) fn reload_theme(&mut self) {
        if self.window_state.is_none() {
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
        if let Some(ws) = self.window_state.as_mut() {
            ws.renderer.set_theme(theme.clone());
            ws.terminal.set_theme_colors(
                theme.fg,
                [theme.bg[0], theme.bg[1], theme.bg[2]],
                [theme.cursor[0], theme.cursor[1], theme.cursor[2]],
                theme.palette,
            );
            ws.mark_dirty();
        }
    }

    /// Re-parse `config.toml` and apply whatever actually changed. A bad
    /// TOML parse is logged and the running config is left untouched.
    pub(crate) fn reload_config(&mut self) {
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

        if let Some(ws) = self.window_state.as_mut() {
            if old_config.font.min_contrast != self.config.font.min_contrast {
                ws.renderer.set_min_contrast(self.config.font.min_contrast);
            }
            if old_config.window.background_opacity != self.config.window.background_opacity {
                ws.renderer
                    .set_background_opacity(self.config.window.background_opacity);
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
            let mode = platform::option_as_alt_from_config(self.config.input.macos_option_as_alt);
            self.input.set_option_as_alt(mode);
            if let Some(ws) = self.window_state.as_ref() {
                platform::set_option_as_alt(&ws.window, mode);
            }
        }
        if old_config.cursor.style != self.config.cursor.style
            && let Some(ws) = self.window_state.as_mut()
        {
            ws.terminal
                .set_cursor_shape(vt_shape_from_config(self.config.cursor.style));
        }
        if diff.repaint_only {
            self.mark_dirty();
        }
    }

    /// A file under `themes/` changed on disk. Only re-resolve if it's the
    /// theme actually in use (either a named override in the user dir or an
    /// absolute-path spec pointing at that file).
    pub(crate) fn on_theme_file_changed(&mut self, path: &std::path::Path) {
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
}
