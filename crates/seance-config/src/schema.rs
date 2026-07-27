//! Serde schema for `$XDG_CONFIG_HOME/seance/config.toml`.
//!
//! Each section uses `#[serde(default)]` on both the struct and its fields so a
//! partial config fills in every missing field from the compile-time defaults.
//!
//! The `theme` key is stored as a raw string; resolution into a palette is the
//! job of the `theme` module.

use std::collections::BTreeMap;

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
    /// Per-scheme link handlers, keyed by a glob matched against the whole
    /// link URL. The first key (in the map's sorted order) whose glob matches
    /// wins; an empty map falls back to the platform opener (`open` /
    /// `xdg-open`).
    ///
    /// ```toml
    /// [links.handlers]
    /// "file://*.{rs,toml,md}" = ["nvim", "+{line}", "{path}"]
    /// "https://*"             = "open"
    /// ```
    pub handlers: BTreeMap<String, LinkHandler>,
}

impl Default for LinksConfig {
    fn default() -> Self {
        Self {
            url: true,
            paths: true,
            modifiers: LinkModifiersConfig::default(),
            handlers: BTreeMap::new(),
        }
    }
}

impl LinksConfig {
    /// Resolve the argv a matching handler would launch for `url`, substituting
    /// the `{url}`, `{path}`, `{line}`, and `{col}` placeholders.
    ///
    /// Returns `None` when no handler glob matches, letting the caller fall
    /// back to the platform opener. A token that references `{line}` or `{col}`
    /// while that anchor is absent is dropped, so `"+{line}"` disappears rather
    /// than becoming a bare `"+"`. A template with no placeholder at all gets
    /// `url` appended as a trailing argument (so `"open"` becomes `[open, url]`).
    pub fn resolve_handler(
        &self,
        url: &str,
        path: &str,
        line: Option<u32>,
        col: Option<u32>,
    ) -> Option<Vec<String>> {
        // Match globs against the URL with any `file://` location anchor
        // removed, so `"file://*.rs"` still matches `file:///a/b.rs:42:7`.
        let key = if url.starts_with("file:") {
            strip_file_anchor(url)
        } else {
            url
        };
        let template = self
            .handlers
            .iter()
            .find(|(glob, _)| glob_match(glob, key))
            .map(|(_, handler)| handler.tokens())?;

        let line = line.map(|n| n.to_string());
        let col = col.map(|n| n.to_string());
        let mut argv = Vec::with_capacity(template.len() + 1);
        let mut any_placeholder = false;
        for token in template {
            if (token.contains("{line}") && line.is_none())
                || (token.contains("{col}") && col.is_none())
            {
                any_placeholder = true;
                continue;
            }
            let substituted = token
                .replace("{url}", url)
                .replace("{path}", path)
                .replace("{line}", line.as_deref().unwrap_or(""))
                .replace("{col}", col.as_deref().unwrap_or(""));
            any_placeholder |= substituted != *token;
            argv.push(substituted);
        }
        if !any_placeholder {
            argv.push(url.to_owned());
        }
        Some(argv)
    }
}

/// A link handler command template: either a bare program name or a full argv
/// with `{url}` / `{path}` / `{line}` / `{col}` placeholders.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum LinkHandler {
    Program(String),
    Argv(Vec<String>),
}

impl LinkHandler {
    fn tokens(&self) -> Vec<String> {
        match self {
            Self::Program(program) => vec![program.clone()],
            Self::Argv(argv) => argv.clone(),
        }
    }
}

/// Strip a `#LINE` / `#LLINE` fragment or a trailing `:LINE[:COL]` anchor off a
/// `file://` URL so extension globs match the bare path. The scheme colon is
/// never peeled (its tail is not all-digits), and stripping stops before it
/// would leave an empty base.
fn strip_file_anchor(url: &str) -> &str {
    let mut base = match url.rsplit_once('#') {
        Some((head, frag)) if !head.is_empty() && is_line_fragment(frag) => head,
        _ => url,
    };
    for _ in 0..2 {
        match base.rsplit_once(':') {
            Some((head, tail))
                if !head.is_empty()
                    && !tail.is_empty()
                    && tail.bytes().all(|b| b.is_ascii_digit()) =>
            {
                base = head;
            }
            _ => break,
        }
    }
    base
}

fn is_line_fragment(frag: &str) -> bool {
    let digits = frag.strip_prefix(['L', 'l']).unwrap_or(frag);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// Match `text` against a shell-style glob supporting `*` (any run, including
/// empty) and `{a,b,c}` alternation. Everything else is literal. Brace groups
/// are expanded first, then each expansion is matched with a linear `*` walk.
fn glob_match(pattern: &str, text: &str) -> bool {
    expand_braces(pattern)
        .iter()
        .any(|expanded| star_match(expanded, text))
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_owned()];
    };
    let Some(close_rel) = pattern[open..].find('}') else {
        return vec![pattern.to_owned()];
    };
    let close = open + close_rel;
    let prefix = &pattern[..open];
    let suffix = &pattern[close + 1..];
    let mut out = Vec::new();
    for alt in pattern[open + 1..close].split(',') {
        for tail in expand_braces(suffix) {
            out.push(format!("{prefix}{alt}{tail}"));
        }
    }
    out
}

fn star_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    // Anchor the first literal segment at the start.
    if let Some(stripped) = rest.strip_prefix(parts[0]) {
        rest = stripped;
    } else {
        return false;
    }
    // Anchor the last literal segment at the end.
    let last = parts[parts.len() - 1];
    let Some(mut rest) = rest.strip_suffix(last).map(str::to_owned) else {
        return false;
    };
    // Each interior segment must appear in order.
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(idx) => rest = rest[idx + part.len()..].to_owned(),
            None => return false,
        }
    }
    true
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
