//! Hot-reload handlers: config file and theme file changes. Called from the
//! `UserEvent` branch of the winit event loop.

use seance_config::ConfigDiff;

use crate::app::{App, link_detector_from_config, mux_shape_from_config};
use crate::keybinds::Keybinds;
use crate::platform;
use crate::surface_state::SurfaceState;

impl App {
    /// Push theme colors into the actor-owned VT. Takes `&mut SurfaceState` so
    /// the caller (including `resumed`, where `self.surface` isn't wired yet)
    /// can apply the theme before publishing the SurfaceState.
    pub(crate) fn apply_terminal_theme_to(
        &self,
        surface: &mut SurfaceState,
        theme: &seance_config::Theme,
    ) {
        surface.set_theme_colors(theme);
    }

    /// Re-resolve the currently-configured theme and push it to the renderer.
    /// Bad theme files keep the previous theme live (#13).
    #[tracing::instrument(level = "info", skip_all)]
    pub(crate) fn reload_theme(&mut self) {
        if self.surface.is_none() {
            return;
        }
        let spec = seance_config::theme::ThemeSpec::parse(
            self.config
                .theme
                .as_deref()
                .unwrap_or(seance_config::theme::resolve::DEFAULT_THEME_NAME),
        );
        let theme = match seance_config::theme::try_load_for(&spec, self.appearance) {
            Ok(t) => t,
            Err(err) => {
                tracing::warn!("theme reload skipped: {err}");
                return;
            }
        };
        if let Some(surface) = self.surface.as_mut() {
            surface.renderer.set_theme(theme.clone());
            surface.set_theme_colors(&theme);
            surface.mark_dirty();
        }
    }

    /// React to an OS light/dark appearance change. Only the `light:/dark:`
    /// theme form depends on appearance, so for any other spec this is a
    /// no-op beyond recording the new value.
    #[tracing::instrument(level = "info", skip_all)]
    pub(crate) fn on_appearance_changed(&mut self, appearance: seance_config::Appearance) {
        if self.appearance == appearance {
            return;
        }
        self.appearance = appearance;
        let active = self
            .config
            .theme
            .as_deref()
            .unwrap_or(seance_config::theme::resolve::DEFAULT_THEME_NAME);
        if seance_config::theme::ThemeSpec::parse(active).is_appearance_sensitive() {
            self.reload_theme();
        }
    }

    /// Re-parse `config.toml` and apply whatever actually changed. A bad
    /// TOML parse is logged and the running config is left untouched.
    #[tracing::instrument(level = "info", skip_all)]
    pub(crate) fn reload_config(&mut self) {
        let Some(path) = seance_config::config_file_path() else {
            return;
        };
        let new_config = match seance_config::try_load_from(&path) {
            Ok(c) => c,
            Err(err) => {
                tracing::warn!("config reload skipped: {err}");
                return;
            }
        };
        let old_config = self.config.clone();
        let diff = ConfigDiff::between(&old_config, &new_config);
        if diff.is_empty() {
            self.config = new_config;
            return;
        }

        tracing::info!("config reloaded: {diff:?}");
        self.config = new_config;

        if let Some(surface) = self.surface.as_mut() {
            if old_config.font.min_contrast != self.config.font.min_contrast {
                surface
                    .renderer
                    .set_min_contrast(self.config.font.min_contrast);
            }
            if old_config.window.background_opacity != self.config.window.background_opacity {
                surface
                    .renderer
                    .set_background_opacity(self.config.window.background_opacity);
            }
        }

        if diff.font_size_changed {
            self.font_size = self.config.font.size;
        }
        if diff.font_size_changed
            || diff.font_adjust_cell_height_changed
            || diff.font_adjust_cell_width_changed
        {
            self.apply_font_metrics(
                diff.font_size_changed,
                diff.font_adjust_cell_height_changed,
                diff.font_adjust_cell_width_changed,
            );
        }
        if diff.font_features_changed
            && let Some(surface) = self.surface.as_mut()
        {
            surface
                .renderer
                .set_font_features(&self.config.font.features);
            surface.mark_dirty();
        }
        if diff.font_family_changed {
            tracing::info!(
                "font.family change takes effect on restart (live swap not yet supported)"
            );
        }
        if diff.scrollback_limit_changed {
            tracing::info!(
                "scrollback.limit change takes effect on restart (live swap not yet supported)"
            );
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
            if let Some(surface) = self.surface.as_ref() {
                platform::set_option_as_alt(&surface.window, mode);
            }
        }
        if diff.keybinds_changed {
            self.keybinds = Keybinds::from_config(&self.config.keybind);
        }
        if diff.links_changed
            && let Some(surface) = self.surface.as_mut()
        {
            surface
                .mux
                .set_link_detector(link_detector_from_config(&self.config.links));
            surface.refresh_hovered_link();
        }
        if old_config.cursor.style != self.config.cursor.style
            && let Some(surface) = self.surface.as_mut()
        {
            surface.set_cursor_shape(mux_shape_from_config(self.config.cursor.style));
        }
        if diff.repaint_only {
            self.mark_dirty();
        }
    }

    /// A file under `themes/` changed on disk. Only re-resolve if it's the
    /// theme actually in use (either a named override in the user dir or an
    /// absolute-path spec pointing at that file).
    #[tracing::instrument(level = "info", skip_all)]
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
