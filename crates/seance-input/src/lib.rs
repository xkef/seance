//! Keyboard and mouse input handling for séance.
//!
//! Translates winit key/mouse events into terminal-level actions using
//! libghostty-vt's key and mouse encoders.

mod keymap;

use libghostty_vt::{key, mouse};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, PhysicalKey};

/// An action produced by the input handler in response to a key event.
#[derive(Debug)]
pub enum Action {
    /// Raw bytes to write to the PTY.
    WritePty(Vec<u8>),
    /// Quit the application (Cmd+Q).
    Quit,
    /// Close the current window (Cmd+W).
    CloseWindow,
    /// Copy selection to clipboard (Cmd+C).
    Copy,
    /// Paste from clipboard (Cmd+V).
    Paste,
    /// Select all terminal content (Cmd+A).
    SelectAll,
    /// Increase font size (Cmd+=).
    IncreaseFontSize,
    /// Decrease font size (Cmd+-).
    DecreaseFontSize,
    /// Reset font size to default (Cmd+0).
    ResetFontSize,
    /// No action for this event.
    Ignore,
}

/// Terminal mode flags queried from the VT emulator.
///
/// Shared between the input encoder (which needs cursor_keys, mouse modes)
/// and the app layer (which needs bracketed_paste for paste handling).
#[derive(Debug, Clone, Copy, Default)]
pub struct TerminalModes {
    pub cursor_keys: bool,
    pub mouse_event: i32,
    pub mouse_format_sgr: bool,
    pub synchronized_output: bool,
    pub bracketed_paste: bool,
}

/// Translates winit events into terminal actions.
pub struct InputHandler {
    key_encoder: key::Encoder<'static>,
    mouse_encoder: mouse::Encoder<'static>,
}

impl Default for InputHandler {
    fn default() -> Self {
        Self {
            key_encoder: key::Encoder::new().expect("key encoder"),
            mouse_encoder: mouse::Encoder::new().expect("mouse encoder"),
        }
    }
}

impl InputHandler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Process a keyboard event and return the resulting action.
    pub fn handle_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &winit::event::Modifiers,
        term_modes: TerminalModes,
    ) -> Action {
        if event.state != ElementState::Pressed {
            return Action::Ignore;
        }

        let super_key = modifiers.state().super_key();

        // Cmd shortcuts (macOS).
        if super_key && let Key::Character(c) = &event.logical_key {
            match c.as_str() {
                "q" => return Action::Quit,
                "w" => return Action::CloseWindow,
                "c" => return Action::Copy,
                "v" => return Action::Paste,
                "a" => return Action::SelectAll,
                "+" | "=" => return Action::IncreaseFontSize,
                "-" => return Action::DecreaseFontSize,
                "0" => return Action::ResetFontSize,
                _ => {}
            }
        }

        let bytes = self.encode_key(event, modifiers, term_modes);
        if bytes.is_empty() {
            Action::Ignore
        } else {
            Action::WritePty(bytes)
        }
    }

    /// Encode a mouse wheel event as VT mouse sequences (if mouse tracking
    /// is enabled). Returns `None` when the terminal should handle scrollback
    /// internally instead.
    pub fn encode_mouse_wheel(&mut self, lines: i32, term_modes: TerminalModes) -> Option<Vec<u8>> {
        if term_modes.mouse_event == 0 {
            return None;
        }

        self.mouse_encoder
            .set_tracking_mode(mouse::TrackingMode::Normal);
        if term_modes.mouse_format_sgr {
            self.mouse_encoder.set_format(mouse::Format::Sgr);
        } else {
            self.mouse_encoder.set_format(mouse::Format::X10);
        }

        let mut out = Vec::new();
        let count = lines.unsigned_abs();
        let button = if lines > 0 {
            mouse::Button::Four
        } else {
            mouse::Button::Five
        };

        for _ in 0..count {
            let mut event = mouse::Event::new().ok()?;
            event
                .set_action(mouse::Action::Press)
                .set_button(Some(button))
                .set_position(mouse::Position { x: 0.0, y: 0.0 });
            self.mouse_encoder.encode_to_vec(&event, &mut out).ok()?;
        }

        if out.is_empty() { None } else { Some(out) }
    }

    fn encode_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &winit::event::Modifiers,
        term_modes: TerminalModes,
    ) -> Vec<u8> {
        self.key_encoder
            .set_cursor_key_application(term_modes.cursor_keys);

        let physical_key = match event.physical_key {
            PhysicalKey::Code(code) => keymap::map_keycode(code),
            _ => None,
        };
        let Some(gk) = physical_key else {
            return Vec::new();
        };

        let Ok(mut key_event) = key::Event::new() else {
            return Vec::new();
        };
        key_event
            .set_key(gk)
            .set_action(keymap::map_action(event.state))
            .set_mods(keymap::map_mods(modifiers));

        if let Some(text) = &event.text {
            key_event.set_utf8(Some(text.as_str()));
        }

        let mut buf = Vec::new();
        let _ = self.key_encoder.encode_to_vec(&key_event, &mut buf);
        buf
    }
}
