//! App-level commands produced by global keybinds.
//!
//! Distinct from `seance_input::VtInput`: these are actions the app
//! itself handles (clipboard, font size, window lifecycle), not bytes
//! to forward to the PTY.

use winit::event::{ElementState, KeyEvent, Modifiers};
use winit::keyboard::{Key, NamedKey};

#[derive(Debug, Clone, Copy)]
pub enum AppCommand {
    Quit,
    CloseWindow,
    Copy,
    Paste,
    SelectAll,
    FontSizeDelta(i8),
    FontSizeReset,
    ToggleFullscreen,
}

/// Match a global keybind (macOS Cmd-shortcuts for app lifecycle,
/// clipboard, font size). Consulted before the VT encoder; on a miss the
/// event falls through to `seance_input::InputHandler`.
pub fn match_global_keybind(event: &KeyEvent, modifiers: &Modifiers) -> Option<AppCommand> {
    if event.state != ElementState::Pressed {
        return None;
    }
    if !modifiers.state().super_key() {
        return None;
    }
    match &event.logical_key {
        Key::Character(c) => Some(match c.as_str() {
            "q" => AppCommand::Quit,
            "w" => AppCommand::CloseWindow,
            "c" => AppCommand::Copy,
            "v" => AppCommand::Paste,
            "a" => AppCommand::SelectAll,
            "+" | "=" => AppCommand::FontSizeDelta(1),
            "-" => AppCommand::FontSizeDelta(-1),
            "0" => AppCommand::FontSizeReset,
            _ => return None,
        }),
        Key::Named(NamedKey::Enter) => Some(AppCommand::ToggleFullscreen),
        _ => None,
    }
}
