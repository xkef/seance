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
    #[serde(rename = "shell-integration")]
    pub shell_integration: ShellIntegrationConfig,
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
            shell_integration: ShellIntegrationConfig::default(),
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
            padding_x: 12,
            padding_y: 0,
            decoration: true,
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

/// Which shell `seance-app` injects integration hooks for on PTY spawn.
///
/// `Auto` reads the spawned shell's binary name (see [`Shell::from_binary`]);
/// the explicit variants force one shell regardless of the binary; `None`
/// disables injection entirely so the child launches with an unmodified
/// environment and argv.
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShellIntegrationDetect {
    #[default]
    Auto,
    Bash,
    Zsh,
    Fish,
    Elvish,
    None,
}

/// A shell whose integration drop-in seance ships. This is the resolved
/// target `seance-app` uses to pick the injection strategy (rcfile, ZDOTDIR,
/// XDG_DATA_DIRS, …); the wire-facing knob is [`ShellIntegrationDetect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Elvish,
}

impl Shell {
    /// Resolve a shell from the binary seance is about to spawn. Accepts a
    /// full path (`/bin/zsh`), a bare name (`zsh`), or the login-shell form
    /// (`-zsh`, which `login(1)` passes as argv[0]). Returns `None` for a
    /// shell seance ships no integration for.
    pub fn from_binary(binary: &str) -> Option<Self> {
        let base = binary.rsplit(['/', '\\']).next().unwrap_or(binary);
        let base = base.strip_prefix('-').unwrap_or(base);
        match base {
            "bash" => Some(Self::Bash),
            "zsh" => Some(Self::Zsh),
            "fish" => Some(Self::Fish),
            "elvish" => Some(Self::Elvish),
            _ => None,
        }
    }
}

/// One opt-in integration feature. Mirrors Ghostty's
/// `shell-integration-features`; the shipped drop-in scripts gate each block
/// of escape-sequence emission on the presence of the corresponding entry.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ShellIntegrationFeature {
    /// DECSCUSR bar-at-prompt / block-during-command shaping.
    Cursor,
    /// A `sudo` wrapper that re-exports the integration env across the
    /// privilege boundary so semantic prompts survive.
    Sudo,
    /// OSC 2 window-title updates from cwd / running command.
    Title,
    /// OSC 7 cwd reporting on every prompt.
    Cwd,
    /// OSC 133 semantic prompt / command boundary marks.
    Prompt,
}

/// `[shell-integration]` — controls the auto-injected shell hooks.
///
/// `features` is a set: an omitted key inherits the full default set, while an
/// explicit `features = []` opts out of every feature (the scripts stay
/// sourceable manually). `detect = "none"` disables injection outright.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ShellIntegrationConfig {
    pub detect: ShellIntegrationDetect,
    pub features: Vec<ShellIntegrationFeature>,
}

impl Default for ShellIntegrationConfig {
    fn default() -> Self {
        Self {
            detect: ShellIntegrationDetect::Auto,
            features: vec![
                ShellIntegrationFeature::Cursor,
                ShellIntegrationFeature::Sudo,
                ShellIntegrationFeature::Title,
                ShellIntegrationFeature::Cwd,
                ShellIntegrationFeature::Prompt,
            ],
        }
    }
}

impl ShellIntegrationConfig {
    /// Resolve which shell to inject for, given the binary `seance-app` is
    /// about to spawn. `None` means no injection: either `detect = "none"`, or
    /// `detect = "auto"` did not recognize `spawned`.
    pub fn resolve_shell(&self, spawned: &str) -> Option<Shell> {
        match self.detect {
            ShellIntegrationDetect::None => None,
            ShellIntegrationDetect::Auto => Shell::from_binary(spawned),
            ShellIntegrationDetect::Bash => Some(Shell::Bash),
            ShellIntegrationDetect::Zsh => Some(Shell::Zsh),
            ShellIntegrationDetect::Fish => Some(Shell::Fish),
            ShellIntegrationDetect::Elvish => Some(Shell::Elvish),
        }
    }

    pub fn feature_enabled(&self, feature: ShellIntegrationFeature) -> bool {
        self.features.contains(&feature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_from_binary_handles_path_bare_and_login_forms() {
        assert_eq!(Shell::from_binary("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_binary("bash"), Some(Shell::Bash));
        assert_eq!(Shell::from_binary("-zsh"), Some(Shell::Zsh));
        assert_eq!(Shell::from_binary("/usr/bin/fish"), Some(Shell::Fish));
        assert_eq!(
            Shell::from_binary("/opt/homebrew/bin/elvish"),
            Some(Shell::Elvish)
        );
    }

    #[test]
    fn shell_from_binary_rejects_unknown_shells() {
        assert_eq!(Shell::from_binary("/bin/nu"), None);
        assert_eq!(Shell::from_binary("xonsh"), None);
        assert_eq!(Shell::from_binary(""), None);
    }

    #[test]
    fn resolve_shell_auto_reads_the_spawned_binary() {
        let cfg = ShellIntegrationConfig::default();
        assert_eq!(cfg.resolve_shell("/bin/zsh"), Some(Shell::Zsh));
        assert_eq!(cfg.resolve_shell("/usr/local/bin/nu"), None);
    }

    #[test]
    fn resolve_shell_forced_variant_ignores_the_spawned_binary() {
        let cfg = ShellIntegrationConfig {
            detect: ShellIntegrationDetect::Fish,
            ..ShellIntegrationConfig::default()
        };
        assert_eq!(cfg.resolve_shell("/bin/bash"), Some(Shell::Fish));
    }

    #[test]
    fn resolve_shell_none_never_injects() {
        let cfg = ShellIntegrationConfig {
            detect: ShellIntegrationDetect::None,
            ..ShellIntegrationConfig::default()
        };
        assert_eq!(cfg.resolve_shell("/bin/zsh"), None);
    }

    #[test]
    fn default_enables_every_feature() {
        let cfg = ShellIntegrationConfig::default();
        for feature in [
            ShellIntegrationFeature::Cursor,
            ShellIntegrationFeature::Sudo,
            ShellIntegrationFeature::Title,
            ShellIntegrationFeature::Cwd,
            ShellIntegrationFeature::Prompt,
        ] {
            assert!(
                cfg.feature_enabled(feature),
                "{feature:?} should default on"
            );
        }
    }
}
