//! Serde schema for `$XDG_CONFIG_HOME/seance/config.toml`.
//!
//! Each section uses `#[serde(default)]` on both the struct and its fields so a
//! partial config fills in every missing field from the compile-time defaults.
//!
//! The `theme` key is stored as a raw string; resolution into a palette is the
//! job of the (forthcoming) theme loader — see issue #12.

use serde::Deserialize;

/// Top-level deserialised view of `config.toml`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Theme name from the `theme = "<spec>"` key. `None` means no theme
    /// was set; the loader substitutes the default. See [`crate::theme`]
    /// for the spec grammar.
    pub theme: Option<String>,
    #[allow(missing_docs)]
    pub font: FontConfig,
    #[allow(missing_docs)]
    pub window: WindowConfig,
    #[allow(missing_docs)]
    pub cursor: CursorConfig,
    #[allow(missing_docs)]
    pub clipboard: ClipboardConfig,
    #[allow(missing_docs)]
    pub scrollback: ScrollbackConfig,
    #[allow(missing_docs)]
    pub mouse: MouseConfig,
    #[allow(missing_docs)]
    pub input: InputConfig,
    #[allow(missing_docs)]
    pub links: LinksConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: Some("Catppuccin Frappe".to_string()),
            font: FontConfig::default(),
            window: WindowConfig::default(),
            cursor: CursorConfig::default(),
            clipboard: ClipboardConfig::default(),
            scrollback: ScrollbackConfig::default(),
            mouse: MouseConfig::default(),
            input: InputConfig::default(),
            links: LinksConfig::default(),
        }
    }
}

/// `[font]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    #[allow(missing_docs)]
    pub family: String,
    /// Point size of the primary face.
    pub size: f32,
    /// OpenType feature tags to enable (e.g. `calt`, `liga`).
    pub features: Vec<String>,
    /// Cell-height tweak, expressed as a percentage string like `"10%"`.
    pub adjust_cell_height: Option<String>,
    /// Cell-width tweak, expressed as a percentage string like `"10%"`.
    pub adjust_cell_width: Option<String>,
    /// Minimum WCAG contrast ratio enforced between fg and bg of the
    /// same cell; `1.0` disables the check.
    pub min_contrast: f32,
    /// Ordered fallback face list consulted on glyph miss.
    pub fallback: Vec<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "JetBrainsMono Nerd Font".to_string(),
            size: 14.0,
            features: vec!["calt".to_string(), "liga".to_string()],
            adjust_cell_height: None,
            adjust_cell_width: None,
            min_contrast: 1.1,
            fallback: Vec::new(),
        }
    }
}

/// `[window]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    /// Horizontal padding inside the window, in pixels.
    pub padding_x: u16,
    /// Vertical padding inside the window, in pixels.
    pub padding_y: u16,
    /// Show the OS window decoration / titlebar.
    pub decoration: bool,
    /// Background alpha, `0.0..=1.0`. Values below `1.0` require a
    /// platform-supported transparent surface.
    pub background_opacity: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding_x: 12,
            padding_y: 0,
            decoration: true,
            background_opacity: 1.0,
        }
    }
}

/// Cursor shape selected from `[cursor].style`.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    /// Solid filled block over the cell.
    Block,
    /// Vertical bar at the leading edge of the cell.
    #[default]
    Bar,
    /// Horizontal underline along the bottom of the cell.
    Underline,
}

/// `[cursor]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorConfig {
    #[allow(missing_docs)]
    pub style: CursorStyle,
    /// Blink the cursor when the window has focus.
    pub blink: bool,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            style: CursorStyle::Bar,
            blink: false,
        }
    }
}

/// `[clipboard]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    /// Allow the terminal to read from the system clipboard via OSC 52.
    pub read: bool,
    /// Allow the terminal to write to the system clipboard via OSC 52.
    pub write: bool,
    /// Prompt before pasting input that contains newlines or control bytes.
    pub paste_protection: bool,
    /// Automatically copy the selection to the clipboard on mouse release.
    pub copy_on_select: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            paste_protection: true,
            copy_on_select: false,
        }
    }
}

/// `[scrollback]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollbackConfig {
    /// Maximum number of off-screen rows retained for a pane.
    pub limit: u32,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self { limit: 50_000 }
    }
}

/// `[mouse]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseConfig {
    /// Hide the OS pointer while keys are being pressed.
    pub hide_while_typing: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            hide_while_typing: true,
        }
    }
}

/// Controls whether the macOS Option key is treated as a VT Alt modifier
/// (producing `ESC`-prefixed sequences like readline/vim expect) or passed
/// through to the OS text composer (producing `ø`, `¬`, … per the active
/// keyboard layout).
///
/// Ignored on non-macOS: Alt is always Alt there.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MacosOptionAsAlt {
    /// Both Option keys compose macOS special characters. Default — matches
    /// Ghostty's default and preserves `ø`/`¬`/`–` input.
    #[default]
    None,
    /// Only left-Option sends ESC-prefix; right-Option still composes.
    Left,
    /// Only right-Option sends ESC-prefix; left-Option still composes.
    Right,
    /// Both Option keys send ESC-prefix. Breaks macOS text composition.
    Both,
}

/// `[input]` table.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    #[allow(missing_docs)]
    pub macos_option_as_alt: MacosOptionAsAlt,
}

/// `[links]` table — controls hyperlink detection and the modifier key
/// the user must hold to activate a link.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LinksConfig {
    /// Detect URLs (`https://…`, `mailto:…`, …) in cell text.
    pub url: bool,
    /// Detect filesystem-path-shaped strings in cell text.
    pub paths: bool,
    /// Modifier combination that arms link clicking.
    pub modifiers: LinkModifiersConfig,
}

impl Default for LinksConfig {
    fn default() -> Self {
        Self {
            url: true,
            paths: true,
            modifiers: LinkModifiersConfig::default(),
        }
    }
}

/// Modifier combination that arms link activation. The default is
/// `super+shift` on macOS (so plain Cmd+click still drags the window)
/// and `ctrl+shift` elsewhere.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum LinkModifiersConfig {
    /// macOS Command + Shift (or Super + Shift on other platforms).
    #[serde(rename = "super+shift")]
    SuperShift,
    /// Control + Shift.
    #[serde(rename = "ctrl+shift")]
    CtrlShift,
    /// macOS Command alone (or Super alone on other platforms).
    #[serde(rename = "super")]
    Super,
    /// Control alone.
    #[serde(rename = "ctrl")]
    Ctrl,
}

impl Default for LinkModifiersConfig {
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Self::SuperShift
        } else {
            Self::CtrlShift
        }
    }
}
