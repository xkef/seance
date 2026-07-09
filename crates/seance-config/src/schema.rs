//! Serde schema for `$XDG_CONFIG_HOME/seance/config.toml`.
//!
//! Each section uses `#[serde(default)]` on both the struct and its fields so a
//! partial config fills in every missing field from the compile-time defaults.
//!
//! The `theme` key is stored as a raw string; resolution into a palette is the
//! job of the `theme` module.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: Option<String>,
    pub font: FontConfig,
    pub window: WindowConfig,
    pub cursor: CursorConfig,
    pub clipboard: ClipboardConfig,
    pub scrollback: ScrollbackConfig,
    pub input: InputConfig,
    pub links: LinksConfig,
    pub keybind: Vec<KeybindConfig>,
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
            input: InputConfig::default(),
            links: LinksConfig::default(),
            keybind: Vec::new(),
        }
    }
}

/// One `[[keybind]]` entry. Both fields are required (no `#[serde(default)]`),
/// so a table missing either is a parse error. The `key`/`action` strings are
/// inert here — `seance-app` owns the chord/action grammar and parses them when
/// it builds the binding table, keeping this crate free of any winit dependency.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeybindConfig {
    pub key: String,
    pub action: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
    pub features: Vec<String>,
    pub adjust_cell_height: Option<String>,
    pub adjust_cell_width: Option<String>,
    pub min_contrast: f32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "JetBrainsMono Nerd Font".to_string(),
            size: 14.0,
            features: vec!["calt".to_string(), "liga".to_string()],
            adjust_cell_height: None,
            adjust_cell_width: None,
            min_contrast: 1.0,
        }
    }
}

/// Window chrome preset. Governs the titlebar and the traffic-light /
/// window-control buttons as a unit; `titlebar_style` tunes the titlebar
/// appearance for the non-`System` presets.
///
/// Only `macos` acts on the full three-way distinction. Other platforms
/// collapse it to a single bit — `System` keeps server-side decorations,
/// `Hidden`/`ButtonsOnly` request a borderless window — because there is no
/// cross-platform equivalent of macOS traffic-light buttons.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WindowDecoration {
    /// Native OS chrome: standard titlebar and window-control buttons.
    System,
    /// No titlebar chrome and the window-control buttons hidden too — a
    /// clean, full-bleed content window.
    Hidden,
    /// Titlebar chrome hidden but the traffic-light buttons stay visible
    /// (the iTerm2 / Ghostty look). macOS-only; treated as `Hidden`
    /// elsewhere.
    ButtonsOnly,
}

impl Default for WindowDecoration {
    /// macOS defaults to the full-bleed `Hidden` chrome that seance has always
    /// shipped; other platforms keep native decorations, since they have no
    /// AppKit-style hidden-titlebar path and a borderless window can be
    /// unmovable under some window managers.
    fn default() -> Self {
        if cfg!(target_os = "macos") {
            Self::Hidden
        } else {
            Self::System
        }
    }
}

/// How the titlebar itself is drawn for the `Hidden` / `ButtonsOnly`
/// decoration presets. Ignored under `WindowDecoration::System`, which always
/// gets the opaque native titlebar. macOS-only.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TitlebarStyle {
    /// Opaque, system-drawn titlebar that reserves its own strip above the
    /// content.
    Native,
    /// Titlebar draws transparent and the content view extends underneath it
    /// (`fullSizeContentView`). The default.
    #[default]
    Transparent,
    /// Title text and the titlebar separator suppressed, on top of the
    /// transparent full-size-content treatment.
    Hidden,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub padding_x: u16,
    pub padding_y: u16,
    pub decoration: WindowDecoration,
    pub titlebar_style: TitlebarStyle,
    pub background_opacity: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding_x: 12,
            padding_y: 0,
            decoration: WindowDecoration::default(),
            titlebar_style: TitlebarStyle::default(),
            background_opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    Block,
    #[default]
    Bar,
    Underline,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CursorConfig {
    pub style: CursorStyle,
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

/// How OSC 52 clipboard reads/writes are authorized.
///
/// The `Ask` variant exists in the wire format so configs can opt in to a
/// confirm-prompt today and have the runtime upgrade them automatically once
/// the modal-overlay UI lands (tracked under the M3 clipboard epic, #6).
/// Until that ships, the runtime treats `Ask` as `Deny` and logs a one-shot
/// hint pointing users at `allow`.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ClipboardPolicy {
    Allow,
    Ask,
    /// Default: silently refuse OSC 52 traffic. Users opt in to clipboard
    /// integration explicitly via `clipboard.{read,write} = "allow"`.
    #[default]
    Deny,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    pub read: ClipboardPolicy,
    pub write: ClipboardPolicy,
    pub paste_protection: bool,
    pub copy_on_select: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            read: ClipboardPolicy::Deny,
            write: ClipboardPolicy::Deny,
            paste_protection: true,
            copy_on_select: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollbackConfig {
    pub limit: u32,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self { limit: 50_000 }
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct InputConfig {
    pub macos_option_as_alt: MacosOptionAsAlt,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LinksConfig {
    pub url: bool,
    pub paths: bool,
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
pub enum LinkModifiersConfig {
    #[serde(rename = "super+shift")]
    SuperShift,
    #[serde(rename = "ctrl+shift")]
    CtrlShift,
    #[serde(rename = "super")]
    Super,
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
