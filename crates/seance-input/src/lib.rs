//! Keyboard and mouse input handling for séance.
//!
//! Pure translation layer: winit events → VT escape sequences via
//! libghostty-vt's key and mouse encoders. App-level keybinds (Cmd+Q,
//! clipboard, font size) are the app's concern and are matched
//! upstream before reaching the encoder.

mod keymap;

use libghostty_vt::{key, mouse};
use seance_vt::TerminalModes;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::PhysicalKey;

/// The result of encoding a VT-bound key event.
#[derive(Debug)]
pub enum VtInput {
    /// Raw bytes to write to the PTY.
    Write(Vec<u8>),
    /// The event produced nothing to forward to the PTY.
    Ignore,
}

/// Translates winit events into VT bytes via libghostty-vt.
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

    /// Encode a key event as VT bytes (cursor keys, function keys, etc.).
    pub fn handle_key(
        &mut self,
        event: &KeyEvent,
        modifiers: &winit::event::Modifiers,
        term_modes: TerminalModes,
    ) -> VtInput {
        if event.state != ElementState::Pressed {
            return VtInput::Ignore;
        }

        let bytes = self.encode_key(event, modifiers, term_modes);
        if bytes.is_empty() {
            VtInput::Ignore
        } else {
            VtInput::Write(bytes)
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
