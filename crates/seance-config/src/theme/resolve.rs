//! Theme name resolution: `theme = "<spec>"` → [`Theme`].
//!
//! Spec forms (Ghostty parity):
//! - `"<Name>"` — look in `$XDG_CONFIG_HOME/seance/themes/<Name>` first,
//!   then fall back to the embedded bundled themes.
//! - `"light:A,dark:B"` — parse both and pick the half matching the current
//!   OS [`Appearance`]. [`load_for`]/[`try_load_for`] take the appearance;
//!   the appearance-free [`load`]/[`try_load`] default to the dark variant.
//! - `"/abs/path"` — load that file directly; error if missing.

use std::fs;
use std::path::{Path, PathBuf};

use super::{Theme, bundled, parse_source, parser::ParseError};
use crate::config_dir;

/// The default theme used when `config.toml` does not set `theme`.
/// Catppuccin Frappe matches the historical look seance shipped with.
pub const DEFAULT_THEME_NAME: &str = "Catppuccin Frappe";

/// OS appearance, used to choose between the `light:`/`dark:` halves of a
/// [`ThemeSpec::LightDark`] spec. Other spec forms ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Appearance {
    Light,
    /// Default. Matches the historical always-dark resolution and the
    /// fallback when the OS reports no preference.
    #[default]
    Dark,
}

/// Parsed spec matching one of the Ghostty `theme = ...` forms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeSpec {
    /// Look up a name in user dir, then bundled.
    Named(String),
    /// `light:A,dark:B` — the resolved half follows the OS [`Appearance`].
    LightDark { light: String, dark: String },
    /// Absolute or explicit filesystem path.
    Path(PathBuf),
}

impl ThemeSpec {
    /// Parse a raw `theme =` value into a spec.
    pub fn parse(raw: &str) -> Self {
        let s = raw.trim();
        if Path::new(s).is_absolute() {
            return Self::Path(PathBuf::from(s));
        }
        if let Some((a, b)) = split_light_dark(s) {
            return Self::LightDark { light: a, dark: b };
        }
        Self::Named(s.to_string())
    }

    /// Whether the resolved palette depends on OS appearance — true only for
    /// the `light:/dark:` form. Callers use this to decide whether an OS
    /// appearance change requires re-resolving the theme.
    pub fn is_appearance_sensitive(&self) -> bool {
        matches!(self, Self::LightDark { .. })
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
            Self::NotFound(name) => {
                write!(f, "theme '{name}' not found in user dir or bundled themes")
            }
            Self::Io(p, e) => write!(f, "reading theme file {}: {e}", p.display()),
            Self::Parse(src, e) => write!(f, "parsing theme {src}: {e}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Resolve the configured `theme` value (or fall back to the default name)
/// into a fully-parsed [`Theme`]. Logs on failure and falls back to the
/// bundled default so the terminal still launches.
pub fn load(spec: Option<&str>) -> Theme {
    load_for(spec, Appearance::default())
}

/// Like [`load`], but picks the `light:`/`dark:` half of a `LightDark` spec
/// according to `appearance`. Named and path specs ignore it.
pub fn load_for(spec: Option<&str>, appearance: Appearance) -> Theme {
    let spec = ThemeSpec::parse(spec.unwrap_or(DEFAULT_THEME_NAME));
    try_load_for(&spec, appearance).unwrap_or_else(|err| {
        tracing::warn!("theme load failed ({err}); falling back to {DEFAULT_THEME_NAME}");
        fallback_bundled(DEFAULT_THEME_NAME)
    })
}

/// Lower-level entrypoint that surfaces errors instead of logging. Useful
/// in tests and in the hot-reload path (#13) where the caller may want to
/// reject a bad edit rather than silently fall back. A `LightDark` spec
/// resolves its dark half; use [`try_load_for`] to honor OS appearance.
pub fn try_load(spec: &ThemeSpec) -> Result<Theme, LoadError> {
    try_load_for(spec, Appearance::default())
}

/// Like [`try_load`], but resolves a `LightDark` spec's half from
/// `appearance`. Named and path specs ignore it.
pub fn try_load_for(spec: &ThemeSpec, appearance: Appearance) -> Result<Theme, LoadError> {
    match spec {
        ThemeSpec::Named(name) => load_named(name),
        ThemeSpec::LightDark { light, dark } => match appearance {
            Appearance::Light => load_named(light),
            Appearance::Dark => load_named(dark),
        },
        ThemeSpec::Path(path) => load_path(path),
    }
}

fn load_named(name: &str) -> Result<Theme, LoadError> {
    if let Some(dir) = config_dir() {
        let user_path = dir.join("themes").join(name);
        match fs::read_to_string(&user_path) {
            Ok(text) => {
                tracing::info!("theme: using user override {}", user_path.display());
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
    // If even the default bundled theme can't be parsed, the vendor dir
    // is broken. We still return *something* so the app starts and the
    // user sees a terminal (just with xterm colors).
    bundled::get(name)
        .and_then(|t| parse_source(t).ok())
        .unwrap_or_else(|| {
            tracing::error!(
                "default bundled theme '{name}' missing or unparseable — \
                 run tools/setup-themes.sh"
            );
            Theme::blank()
        })
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

    #[test]
    fn appearance_defaults_to_dark() {
        assert_eq!(Appearance::default(), Appearance::Dark);
    }

    #[test]
    fn load_for_dark_matches_appearance_free_load() {
        let spec = "light:Catppuccin Latte,dark:Catppuccin Frappe";
        assert_eq!(
            load_for(Some(spec), Appearance::Dark).bg,
            load(Some(spec)).bg
        );
    }

    #[test]
    fn load_for_light_picks_light_variant() {
        let spec = "light:Catppuccin Latte,dark:Catppuccin Frappe";
        // The light half resolves to the named light theme, distinct from
        // the dark half — proving appearance actually switches the palette.
        assert_eq!(
            load_for(Some(spec), Appearance::Light).bg,
            load(Some("Catppuccin Latte")).bg
        );
        assert_ne!(
            load_for(Some(spec), Appearance::Light).bg,
            load_for(Some(spec), Appearance::Dark).bg
        );
    }

    #[test]
    fn load_for_named_ignores_appearance() {
        assert_eq!(
            load_for(Some("Catppuccin Frappe"), Appearance::Light).bg,
            load_for(Some("Catppuccin Frappe"), Appearance::Dark).bg
        );
    }

    #[test]
    fn is_appearance_sensitive_only_for_light_dark() {
        assert!(
            ThemeSpec::parse("light:Catppuccin Latte,dark:Catppuccin Frappe")
                .is_appearance_sensitive()
        );
        assert!(!ThemeSpec::parse("Catppuccin Frappe").is_appearance_sensitive());
        assert!(!ThemeSpec::parse("/etc/theme").is_appearance_sensitive());
    }
}
