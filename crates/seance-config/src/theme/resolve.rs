//! Theme name resolution: `theme = "<spec>"` → [`Theme`].
//!
//! Spec forms (Ghostty parity):
//! - `"<Name>"` — look in `$XDG_CONFIG_HOME/seance/themes/<Name>` first,
//!   then fall back to the embedded bundled themes.
//! - `"light:A,dark:B"` — parse both and pick the dark variant for now.
//!   OS-appearance-driven switching is tracked as a follow-up (#131).
//! - `"/abs/path"` — load that file directly; error if missing.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Theme, bundled, parse_source, parser::ParseError};
use crate::config_dir;

/// The default theme used when `config.toml` does not set `theme`.
/// Catppuccin Frappe matches the historical look seance shipped with.
pub const DEFAULT_THEME_NAME: &str = "Catppuccin Frappe";

/// Parsed spec matching one of the Ghostty `theme = ...` forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSpec {
    /// Look up a name in user dir, then bundled.
    Named(String),
    /// `light:A,dark:B` — we currently always pick `dark`.
    LightDark { light: String, dark: String },
    /// Absolute or explicit filesystem path.
    Path(PathBuf),
}

impl ThemeSpec {
    /// Parse a raw `theme =` value into a spec.
    pub fn parse(raw: &str) -> Self {
        let s = raw.trim();
        if Path::new(s).is_absolute() {
            return ThemeSpec::Path(PathBuf::from(s));
        }
        if let Some((a, b)) = split_light_dark(s) {
            return ThemeSpec::LightDark { light: a, dark: b };
        }
        ThemeSpec::Named(s.to_string())
    }
}

fn split_light_dark(s: &str) -> Option<(String, String)> {
    // Accept either `light:A,dark:B` or `dark:A,light:B`, whitespace trimmed.
    let (left, right) = s.split_once(',')?;
    let (lk, lv) = left.split_once(':')?;
    let (rk, rv) = right.split_once(':')?;
    let (lk, lv, rk, rv) = (lk.trim(), lv.trim(), rk.trim(), rv.trim());
    match (lk, rk) {
        ("light", "dark") => Some((lv.to_string(), rv.to_string())),
        ("dark", "light") => Some((rv.to_string(), lv.to_string())),
        _ => None,
    }
}

/// Errors from [`load`]. Callers typically log and fall back.
#[derive(Debug)]
pub enum LoadError {
    /// A named theme could not be found in the user dir or the bundled set.
    NotFound(String),
    /// An explicit path did not exist or could not be read.
    Io(PathBuf, std::io::Error),
    /// Theme file contained invalid syntax.
    Parse(String, ParseError),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::NotFound(name) => {
                write!(f, "theme '{name}' not found in user dir or bundled themes")
            }
            LoadError::Io(p, e) => write!(f, "reading theme file {}: {e}", p.display()),
            LoadError::Parse(src, e) => write!(f, "parsing theme {src}: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Resolve the configured `theme` value (or fall back to the default name)
/// into a fully-parsed [`Theme`]. Logs on failure and falls back to the
/// bundled default so the terminal still launches.
pub fn load(spec: Option<&str>) -> Theme {
    let spec = ThemeSpec::parse(spec.unwrap_or(DEFAULT_THEME_NAME));
    match try_load(&spec) {
        Ok(t) => t,
        Err(err) => {
            log::warn!("theme load failed ({err}); falling back to {DEFAULT_THEME_NAME}");
            fallback_bundled(DEFAULT_THEME_NAME)
        }
    }
}

/// Lower-level entrypoint that surfaces errors instead of logging. Useful
/// in tests and in the forthcoming hot-reload path (#13) where the caller
/// may want to reject a bad edit rather than silently fall back.
pub fn try_load(spec: &ThemeSpec) -> Result<Theme, LoadError> {
    match spec {
        ThemeSpec::Named(name) => load_named(name),
        ThemeSpec::LightDark { dark, .. } => load_named(dark),
        ThemeSpec::Path(path) => load_path(path),
    }
}

fn load_named(name: &str) -> Result<Theme, LoadError> {
    if let Some(dir) = config_dir() {
        let user_path = dir.join("themes").join(name);
        match fs::read_to_string(&user_path) {
            Ok(text) => {
                log::info!("theme: using user override {}", user_path.display());
                return parse_source(&text).map_err(|e| LoadError::Parse(name.to_string(), e));
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(LoadError::Io(user_path, err)),
        }
    }
    let text = bundled::get(name).ok_or_else(|| LoadError::NotFound(name.to_string()))?;
    parse_source(text).map_err(|e| LoadError::Parse(name.to_string(), e))
}

fn load_path(path: &Path) -> Result<Theme, LoadError> {
    let text = fs::read_to_string(path).map_err(|e| LoadError::Io(path.to_path_buf(), e))?;
    parse_source(&text).map_err(|e| LoadError::Parse(path.display().to_string(), e))
}

fn fallback_bundled(name: &str) -> Theme {
    match bundled::get(name).and_then(|t| parse_source(t).ok()) {
        Some(t) => t,
        // If even the default bundled theme can't be parsed, the vendor dir
        // is broken. We still return *something* so the app starts and the
        // user sees a terminal (just with xterm colors).
        None => {
            log::error!(
                "default bundled theme '{name}' missing or unparseable — \
                 run tools/setup-themes.sh"
            );
            Theme::blank()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_named_spec() {
        assert_eq!(
            ThemeSpec::parse("Catppuccin Frappe"),
            ThemeSpec::Named("Catppuccin Frappe".to_string())
        );
    }

    #[test]
    fn parses_absolute_path() {
        match ThemeSpec::parse("/etc/theme") {
            ThemeSpec::Path(p) => assert_eq!(p, PathBuf::from("/etc/theme")),
            other => panic!("expected Path, got {other:?}"),
        }
    }

    #[test]
    fn parses_light_dark_in_either_order() {
        assert_eq!(
            ThemeSpec::parse("light:Rose Pine Dawn,dark:Rose Pine"),
            ThemeSpec::LightDark {
                light: "Rose Pine Dawn".to_string(),
                dark: "Rose Pine".to_string(),
            }
        );
        assert_eq!(
            ThemeSpec::parse("dark:Rose Pine, light:Rose Pine Dawn"),
            ThemeSpec::LightDark {
                light: "Rose Pine Dawn".to_string(),
                dark: "Rose Pine".to_string(),
            }
        );
    }

    #[test]
    fn load_none_yields_default() {
        // Runs against the embedded bundled set.
        let t = load(None);
        // default Catppuccin Frappe bg.
        assert_eq!(t.bg, [0x30, 0x34, 0x46, 0xff]);
    }

    #[test]
    fn load_missing_name_falls_back() {
        let t = load(Some("Definitely Not A Real Theme 9000"));
        // Falls back to default — so we still get Catppuccin Frappe's bg.
        assert_eq!(t.bg, [0x30, 0x34, 0x46, 0xff]);
    }

    #[test]
    fn load_light_dark_picks_dark() {
        let t = load(Some("light:Catppuccin Latte,dark:Catppuccin Frappe"));
        assert_eq!(t.bg, [0x30, 0x34, 0x46, 0xff]);
    }
}
