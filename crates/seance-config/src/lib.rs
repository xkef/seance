//! Configuration loading for seance.
//!
//! Reads `$XDG_CONFIG_HOME/seance/config.toml` (falling back to
//! `$HOME/.config/seance/config.toml`). Missing file → compile-time defaults.
//! Parse error → log and return defaults so the terminal still launches.
//!
//! Theme files live alongside the config but use Ghostty's config syntax (not
//! TOML) and are resolved by a separate module (issue #12).

mod schema;
pub mod theme;

pub use schema::{
    ClipboardConfig, Config, CursorConfig, CursorStyle, FontConfig, MouseConfig, ScrollbackConfig,
    WindowConfig,
};
pub use theme::{Theme, load as load_theme};

use std::path::{Path, PathBuf};
use std::{env, fs};

/// Filename of the main config file within the seance config directory.
pub const CONFIG_FILENAME: &str = "config.toml";

/// Return `$XDG_CONFIG_HOME/seance/` (or `$HOME/.config/seance/`), without
/// creating it. Returns `None` if neither env var is set.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(xdg) = env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(xdg).join("seance"));
    }
    env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(|home| PathBuf::from(home).join(".config").join("seance"))
}

/// Path to `config.toml` in the resolved config directory.
pub fn config_file_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join(CONFIG_FILENAME))
}

/// Load the config. Missing file or parse error yields defaults.
pub fn load() -> Config {
    match config_file_path() {
        Some(path) => load_from(&path),
        None => {
            log::debug!("no XDG_CONFIG_HOME or HOME set; using compile-time config defaults");
            Config::default()
        }
    }
}

/// Load from an explicit path. Logs and returns defaults on any failure.
pub fn load_from(path: &Path) -> Config {
    match fs::read_to_string(path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(cfg) => {
                log::info!("loaded config from {}", path.display());
                cfg
            }
            Err(err) => {
                log::warn!("failed to parse {} — using defaults: {err}", path.display());
                Config::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("no config file at {} — using defaults", path.display());
            Config::default()
        }
        Err(err) => {
            log::warn!("failed to read {} — using defaults: {err}", path.display());
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_yields_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        let def = Config::default();
        assert_eq!(cfg.font.family, def.font.family);
        assert_eq!(cfg.font.size, def.font.size);
        assert_eq!(cfg.window.padding_x, 0);
        assert_eq!(cfg.cursor.style, CursorStyle::Block);
        assert!(cfg.theme.is_none());
    }

    #[test]
    fn partial_font_section_fills_other_fields() {
        let cfg: Config = toml::from_str(
            r#"
            theme = "Catppuccin Mocha"
            [font]
            family = "Berkeley Mono"
            size = 16.0
            "#,
        )
        .unwrap();
        assert_eq!(cfg.theme.as_deref(), Some("Catppuccin Mocha"));
        assert_eq!(cfg.font.family, "Berkeley Mono");
        assert_eq!(cfg.font.size, 16.0);
        assert_eq!(cfg.font.min_contrast, 1.0);
        assert!(cfg.font.features.is_empty());
    }

    #[test]
    fn cursor_style_enum_is_lowercase() {
        let cfg: Config = toml::from_str(
            r#"
            [cursor]
            style = "bar"
            blink = true
            "#,
        )
        .unwrap();
        assert_eq!(cfg.cursor.style, CursorStyle::Bar);
        assert!(cfg.cursor.blink);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let err = toml::from_str::<Config>(
            r#"
            [font]
            nonsense = 1
            "#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("nonsense"), "{err}");
    }

    #[test]
    fn clipboard_defaults_match_spec() {
        let cfg = Config::default();
        assert!(cfg.clipboard.read);
        assert!(cfg.clipboard.write);
        assert!(cfg.clipboard.paste_protection);
        assert!(!cfg.clipboard.copy_on_select);
    }

    #[test]
    fn config_dir_honors_xdg_over_home() {
        // Save + restore env around the test.
        // Note: unsafe{} because std::env::set_var is `unsafe` on edition 2024.
        let saved_xdg = env::var_os("XDG_CONFIG_HOME");
        let saved_home = env::var_os("HOME");
        unsafe {
            env::set_var("XDG_CONFIG_HOME", "/tmp/seance-test-xdg");
            env::set_var("HOME", "/tmp/seance-test-home");
        }
        assert_eq!(
            config_dir(),
            Some(PathBuf::from("/tmp/seance-test-xdg/seance"))
        );
        unsafe {
            env::remove_var("XDG_CONFIG_HOME");
        }
        assert_eq!(
            config_dir(),
            Some(PathBuf::from("/tmp/seance-test-home/.config/seance"))
        );
        unsafe {
            match saved_xdg {
                Some(v) => env::set_var("XDG_CONFIG_HOME", v),
                None => env::remove_var("XDG_CONFIG_HOME"),
            }
            match saved_home {
                Some(v) => env::set_var("HOME", v),
                None => env::remove_var("HOME"),
            }
        }
    }
}
