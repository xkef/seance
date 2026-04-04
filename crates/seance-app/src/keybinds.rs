//! Global keybinds that take precedence over VT input encoding.
//!
//! Today: macOS Cmd-shortcuts for app lifecycle, clipboard, font size.
//! Consulted before the VT encoder; on a miss the event falls through
//! to `seance_input::InputHandler`.

use winit::event::{ElementState, KeyEvent, Modifiers};
use winit::keyboard::Key;

use crate::command::AppCommand;

#[derive(Default)]
pub struct Keybinds;

impl Keybinds {
    pub fn new() -> Self {
        Self
    }

    pub fn match_event(&self, event: &KeyEvent, modifiers: &Modifiers) -> Option<AppCommand> {
        if event.state != ElementState::Pressed {
            return None;
        }
        if !modifiers.state().super_key() {
            return None;
        }
        let Key::Character(c) = &event.logical_key else {
            return None;
        };
        Some(match c.as_str() {
            "q" => AppCommand::Quit,
            "w" => AppCommand::CloseWindow,
            "c" => AppCommand::Copy,
            "v" => AppCommand::Paste,
            "a" => AppCommand::SelectAll,
            "+" | "=" => AppCommand::FontSizeDelta(1),
            "-" => AppCommand::FontSizeDelta(-1),
            "0" => AppCommand::FontSizeReset,
            _ => return None,
        })
    }
}
