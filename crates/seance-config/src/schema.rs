//! Serde schema for `$XDG_CONFIG_HOME/seance/config.toml`.
//!
//! Each section uses `#[serde(default)]` on both the struct and its fields so a
//! partial config fills in every missing field from the compile-time defaults.
//!
//! The `theme` key is stored as a raw string; resolution into a palette is the
//! job of the (forthcoming) theme loader — see issue #12.

use serde::Deserialize;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub theme: Option<String>,
    pub font: FontConfig,
    pub window: WindowConfig,
    pub cursor: CursorConfig,
    pub clipboard: ClipboardConfig,
    pub scrollback: ScrollbackConfig,
    pub mouse: MouseConfig,
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
    pub fallback: Vec<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "JetBrainsMono Nerd Font".to_string(),
            size: 14.0,
            features: Vec::new(),
            adjust_cell_height: None,
            adjust_cell_width: None,
            min_contrast: 1.0,
            fallback: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub padding_x: u16,
    pub padding_y: u16,
    pub decoration: bool,
    pub background_opacity: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            padding_x: 0,
            padding_y: 0,
            decoration: true,
            background_opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    #[default]
    Block,
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
            style: CursorStyle::Block,
            blink: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClipboardConfig {
    pub read: bool,
    pub write: bool,
    pub paste_protection: bool,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScrollbackConfig {
    pub limit: u32,
}

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self { limit: 10_000 }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseConfig {
    pub hide_while_typing: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            hide_while_typing: true,
        }
    }
}
